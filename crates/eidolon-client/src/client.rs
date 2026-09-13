//! Authoritative client networking, 3-way cryptographic handshake, and packet dispatch.

use std::net::{SocketAddr, UdpSocket};
use std::time::Instant;

use eidolon_core::fixed::Vec3Fix;
use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw};
use eidolon_net::auth::{
    compute_client_proof, ConnectChallengeRequest, ConnectChallengeResponse,
    ConnectFinalizeRequest, ConnectFinalizeResponse, CHALLENGE_RESP_LEN, FINALIZE_RESP_LEN,
    NONCE_LEN,
};
use eidolon_net::error::NetError;
use eidolon_net::packet::PacketHeader;
use eidolon_net::protocol::{
    ChannelType, PacketType, HEADER_SIZE, MAX_PACKET_SIZE, PROTOCOL_VERSION,
};

use crate::config::ClientConfig;
use crate::event::ClientEvent;
use crate::world_view::{ClientTransform, ClientWorldView};

/// Client errors during network operations and cryptographic handshake.
#[derive(Debug)]
pub enum ClientError {
    /// Underlying I/O error on UDP socket.
    Io(std::io::Error),
    /// Protocol or serialization error.
    Net(NetError),
    /// Action attempted while client is not in connected state.
    NotConnected,
    /// Connection handshake timed out waiting for server response.
    HandshakeTimeout,
    /// Cryptographic proof verification failed.
    AuthenticationFailed(&'static str),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Net(e) => write!(f, "Network protocol error: {e}"),
            Self::NotConnected => write!(f, "Client is not connected to server"),
            Self::HandshakeTimeout => write!(f, "Connection handshake timed out"),
            Self::AuthenticationFailed(msg) => write!(f, "Authentication failed: {msg}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<NetError> for ClientError {
    fn from(err: NetError) -> Self {
        Self::Net(err)
    }
}

/// Internal connection state machine for `EidolonClient`.
#[derive(Debug)]
enum ConnectionState {
    Disconnected,
    ConnectingChallenge {
        client_nonce: [u8; NONCE_LEN],
        sent_at: Instant,
    },
    #[allow(dead_code)]
    ConnectingFinalize {
        auth_cookie: [u8; 32],
        client_nonce: [u8; NONCE_LEN],
        server_nonce: [u8; NONCE_LEN],
        sent_at: Instant,
    },
    Connected {
        session_id: u64,
        authority_epoch: u32,
    },
}

/// High-level native game client runtime.
pub struct EidolonClient {
    socket: UdpSocket,
    config: ClientConfig,
    state: ConnectionState,
    world_view: ClientWorldView,
    send_seq: u16,
    recv_seq: u16,
    ack_mask: u32,
    packet_buffer: [u8; MAX_PACKET_SIZE],
}

impl EidolonClient {
    /// Returns the session ID assigned by the server if connected.
    pub fn session_id(&self) -> Option<u64> {
        match self.state {
            ConnectionState::Connected { session_id, .. } => Some(session_id),
            _ => None,
        }
    }

    /// Returns the active authority epoch if connected.
    pub fn authority_epoch(&self) -> Option<u32> {
        match self.state {
            ConnectionState::Connected {
                authority_epoch, ..
            } => Some(authority_epoch),
            _ => None,
        }
    }

    /// Creates and initializes a new `EidolonClient` bound to a non-blocking UDP socket.
    pub fn new(config: ClientConfig) -> Result<Self, ClientError> {
        let bind_addr = config
            .local_bind_addr
            .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0)));
        let socket = UdpSocket::bind(bind_addr)?;
        socket.set_nonblocking(true)?;

        Ok(Self {
            socket,
            config,
            state: ConnectionState::Disconnected,
            world_view: ClientWorldView::new(),
            send_seq: 0,
            recv_seq: 0,
            ack_mask: 0,
            packet_buffer: [0u8; MAX_PACKET_SIZE],
        })
    }

    /// Initiates connection and cryptographic challenge handshake with the server.
    pub fn connect(&mut self) -> Result<(), ClientError> {
        let mut client_nonce = [0u8; NONCE_LEN];
        // Deterministic pseudo-random entropy derived from current time
        let now_nanos = Instant::now().elapsed().as_nanos();
        for (i, byte) in client_nonce.iter_mut().enumerate() {
            *byte = ((now_nanos >> (i * 4)) ^ (self.config.account_id.0 as u128 >> (i * 3))) as u8;
        }

        let challenge_req = ConnectChallengeRequest {
            account_id: self.config.account_id.0,
            client_nonce,
            protocol_version: PROTOCOL_VERSION,
        };

        let mut out_buf = [0u8; 64];
        let req_len = challenge_req.write_to(&mut out_buf)?;

        let header = PacketHeader::new(
            ChannelType::ReliableOrdered,
            PacketType::ReliableMessage,
            self.send_seq,
            self.recv_seq,
            self.ack_mask,
        );
        self.send_seq = self.send_seq.wrapping_add(1);

        let mut wire_buf = [0u8; 128];
        let header_len = header.write_to(&mut wire_buf)?;
        wire_buf[header_len..header_len + req_len].copy_from_slice(&out_buf[..req_len]);

        self.socket
            .send_to(&wire_buf[..header_len + req_len], self.config.server_addr)?;

        self.state = ConnectionState::ConnectingChallenge {
            client_nonce,
            sent_at: Instant::now(),
        };

        Ok(())
    }

    /// Polls incoming UDP packets, advances handshake state machine, and returns queued events.
    pub fn poll_events(&mut self) -> Result<Vec<ClientEvent>, ClientError> {
        let mut events = Vec::new();

        // Check for handshake timeout
        match &self.state {
            ConnectionState::ConnectingChallenge { sent_at, .. }
            | ConnectionState::ConnectingFinalize { sent_at, .. }
                if sent_at.elapsed() > self.config.timeout =>
            {
                self.state = ConnectionState::Disconnected;
                return Err(ClientError::HandshakeTimeout);
            }
            _ => {}
        }

        loop {
            match self.socket.recv_from(&mut self.packet_buffer) {
                Ok((len, peer)) => {
                    if peer != self.config.server_addr {
                        continue;
                    }

                    if len < HEADER_SIZE {
                        continue;
                    }

                    let (header, header_len) =
                        match PacketHeader::read_from(&self.packet_buffer[..len]) {
                            Ok(h) => h,
                            Err(_) => continue,
                        };

                    let payload = &self.packet_buffer[header_len..len];

                    match &self.state {
                        ConnectionState::ConnectingChallenge { client_nonce, .. } => {
                            if payload.len() >= CHALLENGE_RESP_LEN {
                                if let Ok(resp) = ConnectChallengeResponse::read_from(payload) {
                                    let server_nonce = resp.server_nonce;
                                    let auth_cookie = resp.auth_cookie;

                                    // Compute client proof using session ticket credential
                                    let credential = &self.config.session_ticket.0;
                                    let client_proof = compute_client_proof(
                                        credential,
                                        &auth_cookie,
                                        client_nonce,
                                        &server_nonce,
                                    );

                                    let finalize_req = ConnectFinalizeRequest {
                                        account_id: self.config.account_id.0,
                                        auth_cookie,
                                        client_proof,
                                    };

                                    let mut out_finalize = [0u8; 128];
                                    if let Ok(fin_len) = finalize_req.write_to(&mut out_finalize) {
                                        let fin_header = PacketHeader::new(
                                            ChannelType::ReliableOrdered,
                                            PacketType::ReliableMessage,
                                            self.send_seq,
                                            self.recv_seq,
                                            self.ack_mask,
                                        );
                                        self.send_seq = self.send_seq.wrapping_add(1);

                                        let mut wire_fin = [0u8; 160];
                                        if let Ok(hdr_len) = fin_header.write_to(&mut wire_fin) {
                                            wire_fin[hdr_len..hdr_len + fin_len]
                                                .copy_from_slice(&out_finalize[..fin_len]);
                                            let _ = self.socket.send_to(
                                                &wire_fin[..hdr_len + fin_len],
                                                self.config.server_addr,
                                            );

                                            self.state = ConnectionState::ConnectingFinalize {
                                                auth_cookie,
                                                client_nonce: *client_nonce,
                                                server_nonce,
                                                sent_at: Instant::now(),
                                            };
                                        }
                                    }
                                }
                            }
                        }
                        ConnectionState::ConnectingFinalize { .. } => {
                            if payload.len() >= FINALIZE_RESP_LEN {
                                if let Ok(resp) = ConnectFinalizeResponse::read_from(payload) {
                                    self.state = ConnectionState::Connected {
                                        session_id: resp.session_id,
                                        authority_epoch: resp.authority_epoch,
                                    };

                                    events.push(ClientEvent::Connected {
                                        server_version: header.version,
                                        session_id: resp.session_id,
                                    });
                                }
                            }
                        }
                        ConnectionState::Connected { .. } => {
                            self.recv_seq = header.sequence;

                            match header.packet_type {
                                PacketType::StateUpdate => {
                                    Self::process_state_update_payload(
                                        &mut self.world_view,
                                        payload,
                                        &mut events,
                                    );
                                }
                                PacketType::ReliableMessage => {
                                    Self::process_gameplay_event_payload(payload, &mut events);
                                }
                                PacketType::Disconnect => {
                                    self.state = ConnectionState::Disconnected;
                                    self.world_view.clear();
                                    events.push(ClientEvent::Disconnected {
                                        reason: "Server requested disconnect",
                                    });
                                }
                                _ => {}
                            }
                        }
                        ConnectionState::Disconnected => {}
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(e) => {
                    return Err(ClientError::Io(e));
                }
            }
        }

        Ok(events)
    }

    /// Dispatches continuous movement intent to authoritative server.
    pub fn send_movement_intent(
        &mut self,
        vx: f32,
        vz: f32,
        yaw_deg: f32,
        flags: u8,
    ) -> Result<(), ClientError> {
        if !self.is_connected() {
            return Err(ClientError::NotConnected);
        }

        let header = PacketHeader::new(
            ChannelType::UnreliableSequenced,
            PacketType::StateUpdate,
            self.send_seq,
            self.recv_seq,
            self.ack_mask,
        );
        self.send_seq = self.send_seq.wrapping_add(1);

        let mut wire_buf = [0u8; 64];
        let hdr_len = header.write_to(&mut wire_buf)?;

        // Encode intent: vx (f32, 4B), vz (f32, 4B), yaw_deg (f32, 4B), flags (u8, 1B)
        let mut idx = hdr_len;
        wire_buf[idx..idx + 4].copy_from_slice(&vx.to_be_bytes());
        idx += 4;
        wire_buf[idx..idx + 4].copy_from_slice(&vz.to_be_bytes());
        idx += 4;
        wire_buf[idx..idx + 4].copy_from_slice(&yaw_deg.to_be_bytes());
        idx += 4;
        wire_buf[idx] = flags;
        idx += 1;

        self.socket
            .send_to(&wire_buf[..idx], self.config.server_addr)?;
        Ok(())
    }

    /// Dispatches reliable gameplay action (combat attack, item use, interaction).
    pub fn send_action(
        &mut self,
        action_type: u8,
        target_id: u32,
        param: u32,
    ) -> Result<(), ClientError> {
        if !self.is_connected() {
            return Err(ClientError::NotConnected);
        }

        let header = PacketHeader::new(
            ChannelType::ReliableOrdered,
            PacketType::ReliableMessage,
            self.send_seq,
            self.recv_seq,
            self.ack_mask,
        );
        self.send_seq = self.send_seq.wrapping_add(1);

        let mut wire_buf = [0u8; 64];
        let hdr_len = header.write_to(&mut wire_buf)?;

        let mut idx = hdr_len;
        wire_buf[idx] = action_type;
        idx += 1;
        wire_buf[idx..idx + 4].copy_from_slice(&target_id.to_be_bytes());
        idx += 4;
        wire_buf[idx..idx + 4].copy_from_slice(&param.to_be_bytes());
        idx += 4;

        self.socket
            .send_to(&wire_buf[..idx], self.config.server_addr)?;
        Ok(())
    }

    /// Extrapolates an entity's transform to current render frame given delta time in seconds.
    pub fn extrapolate_entity(&self, entity_id: u32, delta_time: f32) -> Option<ClientTransform> {
        self.world_view.extrapolate_entity(entity_id, delta_time)
    }

    /// Returns list of visible entities currently tracked in Area of Interest.
    pub fn get_visible_entities(&self) -> Vec<u32> {
        self.world_view.get_visible_entities()
    }

    /// Returns total count of visible entities in Area of Interest.
    pub fn visible_entity_count(&self) -> usize {
        self.world_view.count()
    }

    /// Checks if the client has established a verified authenticated connection.
    pub fn is_connected(&self) -> bool {
        matches!(self.state, ConnectionState::Connected { .. })
    }

    /// Gracefully terminates connection with the server.
    pub fn disconnect(&mut self) {
        if self.is_connected() {
            let header = PacketHeader::new(
                ChannelType::ReliableOrdered,
                PacketType::Disconnect,
                self.send_seq,
                self.recv_seq,
                self.ack_mask,
            );
            let mut buf = [0u8; HEADER_SIZE];
            if header.write_to(&mut buf).is_ok() {
                let _ = self.socket.send_to(&buf, self.config.server_addr);
            }
        }
        self.state = ConnectionState::Disconnected;
        self.world_view.clear();
    }

    fn process_state_update_payload(
        world_view: &mut ClientWorldView,
        payload: &[u8],
        events: &mut Vec<ClientEvent>,
    ) {
        // Wire entity format: entity_id (4B), cell_x (2B), cell_z (2B), cell_y (2B),
        // quant_x (2B), quant_z (2B), quant_y (2B), yaw (1B), flags (1B), vx (2B), vz (2B) = 22 bytes
        let record_size = 22;
        let mut offset = 0;

        while offset + record_size <= payload.len() {
            let chunk = &payload[offset..offset + record_size];
            let entity_id = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let cell_x = i16::from_be_bytes([chunk[4], chunk[5]]) as i32;
            let cell_z = i16::from_be_bytes([chunk[6], chunk[7]]) as i32;
            let cell_y = i16::from_be_bytes([chunk[8], chunk[9]]) as i32;
            let q_x = u16::from_be_bytes([chunk[10], chunk[11]]);
            let q_z = u16::from_be_bytes([chunk[12], chunk[13]]);
            let q_y = u16::from_be_bytes([chunk[14], chunk[15]]);
            let yaw = QuantizedYaw::from_byte(chunk[16]);
            let flags = chunk[17];
            let vx_raw = i16::from_be_bytes([chunk[18], chunk[19]]);
            let vz_raw = i16::from_be_bytes([chunk[20], chunk[21]]);

            let quant_coord = QuantizedCellCoord::new(q_x, q_y, q_z);
            let velocity = Vec3Fix {
                x: eidolon_core::fixed::Fixed64::from_i32(vx_raw as i32),
                y: eidolon_core::fixed::Fixed64::ZERO,
                z: eidolon_core::fixed::Fixed64::from_i32(vz_raw as i32),
            };

            let is_new = world_view.get_entity(entity_id).is_none();
            world_view.upsert_entity(
                entity_id,
                0, // Default type: entity
                cell_x,
                cell_y,
                cell_z,
                quant_coord,
                yaw,
                flags,
                velocity,
            );

            let global_pos =
                QuantizedCellCoord::dequantize_to_global(cell_x, cell_y, cell_z, quant_coord);
            let (x, y, z) = global_pos.to_f64();

            if is_new {
                events.push(ClientEvent::EntitySpawned {
                    entity_id,
                    entity_type: 0,
                    x: x as f32,
                    y: y as f32,
                    z: z as f32,
                    yaw_deg: yaw.to_degrees() as f32,
                });
            } else {
                events.push(ClientEvent::EntityUpdated {
                    entity_id,
                    x: x as f32,
                    y: y as f32,
                    z: z as f32,
                    yaw_deg: yaw.to_degrees() as f32,
                    flags,
                });
            }

            offset += record_size;
        }
    }

    fn process_gameplay_event_payload(payload: &[u8], events: &mut Vec<ClientEvent>) {
        if payload.is_empty() {
            return;
        }

        let event_type = payload[0];
        match event_type {
            1 => {
                // Combat action: type (1B) + source_id (4B) + target_id (4B) + action (1B) + value (4B) = 14B
                if payload.len() >= 14 {
                    let source_id =
                        u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
                    let target_id =
                        u32::from_be_bytes([payload[5], payload[6], payload[7], payload[8]]);
                    let action = payload[9];
                    let value =
                        u32::from_be_bytes([payload[10], payload[11], payload[12], payload[13]]);

                    events.push(ClientEvent::CombatAction {
                        source_id,
                        target_id,
                        action_type: action,
                        value,
                    });
                }
            }
            // Loot acquired: type (1B) + entity_id (4B) + item_id (4B) + amount (4B) = 13B
            2 if payload.len() >= 13 => {
                let entity_id =
                    u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
                let item_id =
                    u32::from_be_bytes([payload[5], payload[6], payload[7], payload[8]]);
                let amount =
                    u32::from_be_bytes([payload[9], payload[10], payload[11], payload[12]]);

                events.push(ClientEvent::LootAcquired {
                    entity_id,
                    item_id,
                    amount,
                });
            }
            _ => {}
        }
    }
}

impl Drop for EidolonClient {
    fn drop(&mut self) {
        self.disconnect();
    }
}
