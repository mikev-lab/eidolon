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
    time_dilation: f32,
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

    /// Returns the current server-mandated time dilation factor (1.0 = normal, <1.0 = dilated).
    pub fn time_dilation(&self) -> f32 {
        self.time_dilation
    }

    /// Sets the local time dilation factor.
    pub fn set_time_dilation(&mut self, factor: f32) {
        self.time_dilation = factor.clamp(0.01, 1.0);
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
            time_dilation: 1.0,
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

        for event in &events {
            if let ClientEvent::TimeDilationChanged { time_dilation } = event {
                self.time_dilation = *time_dilation;
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

    /// Dispatches high-speed vehicle intent inputs to authoritative server.
    pub fn send_vehicle_intent(
        &mut self,
        vehicle_type: u8,
        throttle: u8,
        steering: i8,
        pitch: i8,
        roll: i8,
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

        let mut idx = hdr_len;
        wire_buf[idx] = 0xFE; // Vehicle intent marker
        idx += 1;
        wire_buf[idx] = vehicle_type;
        idx += 1;
        wire_buf[idx] = throttle;
        idx += 1;
        wire_buf[idx] = steering as u8;
        idx += 1;
        wire_buf[idx] = pitch as u8;
        idx += 1;
        wire_buf[idx] = roll as u8;
        idx += 1;

        self.socket
            .send_to(&wire_buf[..idx], self.config.server_addr)?;
        Ok(())
    }

    /// Queries the hierarchical global coordinate for an entity in the client world view.
    pub fn get_entity_global_coord(
        &self,
        entity_id: u32,
    ) -> Option<eidolon_core::global_coord::GlobalCoord> {
        let ent = self.world_view.get_entity(entity_id)?;
        let pos = ent.global_position();
        Some(eidolon_core::global_coord::GlobalCoord::from_continuous(
            pos.x, pos.y, pos.z,
        ))
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

    /// Dispatches an ability cast request targeting an entity.
    pub fn cast_ability(&mut self, ability_id: u32, target_id: u32) -> Result<(), ClientError> {
        self.send_action(2, target_id, ability_id)
    }

    /// Dispatches an equip item request swapping inventory slot into equipment slot.
    pub fn equip_item(&mut self, inventory_slot: u8, equip_slot: u8) -> Result<(), ClientError> {
        self.send_action(3, inventory_slot as u32, equip_slot as u32)
    }

    /// Dispatches an unequip item request moving gear from equipment slot to inventory.
    pub fn unequip_item(&mut self, equip_slot: u8) -> Result<(), ClientError> {
        self.send_action(4, equip_slot as u32, 0)
    }

    /// Dispatches a chat message to the server for distribution.
    pub fn send_chat(
        &mut self,
        channel: u8,
        target_id: u32,
        message: &str,
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

        let mut wire_buf = [0u8; 512];
        let hdr_len = header.write_to(&mut wire_buf)?;

        let text_bytes = message.as_bytes();
        let text_len = text_bytes.len().min(256) as u16;

        let mut idx = hdr_len;
        wire_buf[idx] = 5; // 5 = chat message action
        idx += 1;
        wire_buf[idx] = channel;
        idx += 1;
        wire_buf[idx..idx + 4].copy_from_slice(&target_id.to_be_bytes());
        idx += 4;
        wire_buf[idx..idx + 2].copy_from_slice(&text_len.to_be_bytes());
        idx += 2;
        wire_buf[idx..idx + text_len as usize].copy_from_slice(&text_bytes[..text_len as usize]);
        idx += text_len as usize;

        self.socket
            .send_to(&wire_buf[..idx], self.config.server_addr)?;
        Ok(())
    }

    /// Dispatches a party management command (1 = invite, 2 = accept, 3 = leave).
    pub fn party_command(&mut self, cmd: u8, target_account: u64) -> Result<(), ClientError> {
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
        wire_buf[idx] = 6; // 6 = party command
        idx += 1;
        wire_buf[idx] = cmd;
        idx += 1;
        wire_buf[idx..idx + 8].copy_from_slice(&target_account.to_be_bytes());
        idx += 8;

        self.socket
            .send_to(&wire_buf[..idx], self.config.server_addr)?;
        Ok(())
    }

    /// Dispatches a building placement request (modular prefab or freeform piece).
    pub fn place_structure(
        &mut self,
        prefab_type_id: u32,
        x: f32,
        y: f32,
        z: f32,
        yaw_degrees: f32,
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
        wire_buf[idx] = 6; // 6 = place structure action
        idx += 1;
        wire_buf[idx..idx + 4].copy_from_slice(&prefab_type_id.to_be_bytes());
        idx += 4;
        wire_buf[idx..idx + 4].copy_from_slice(&x.to_be_bytes());
        idx += 4;
        wire_buf[idx..idx + 4].copy_from_slice(&y.to_be_bytes());
        idx += 4;
        wire_buf[idx..idx + 4].copy_from_slice(&z.to_be_bytes());
        idx += 4;
        wire_buf[idx..idx + 4].copy_from_slice(&yaw_degrees.to_be_bytes());
        idx += 4;

        self.socket
            .send_to(&wire_buf[..idx], self.config.server_addr)?;
        Ok(())
    }

    /// Dispatches a structure demolition request.
    pub fn destroy_structure(&mut self, structure_id: u32) -> Result<(), ClientError> {
        self.send_action(7, structure_id, 0)
    }

    /// Dispatches an interior cell entry request.
    pub fn enter_interior_cell(&mut self, cell_id: u32) -> Result<(), ClientError> {
        self.send_action(8, cell_id, 0)
    }

    /// Dispatches an interior cell exit request back to the open world.
    pub fn exit_interior_cell(&mut self) -> Result<(), ClientError> {
        self.send_action(9, 0, 0)
    }

    /// Dispatches a decorative interior item move / update command.
    pub fn move_interior_item(
        &mut self,
        cell_id: u32,
        item_instance_id: u32,
        local_x: f32,
        local_y: f32,
        local_z: f32,
        yaw_degrees: f32,
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
        wire_buf[idx] = 10; // 10 = move interior item
        idx += 1;
        wire_buf[idx..idx + 4].copy_from_slice(&cell_id.to_be_bytes());
        idx += 4;
        wire_buf[idx..idx + 4].copy_from_slice(&item_instance_id.to_be_bytes());
        idx += 4;
        wire_buf[idx..idx + 4].copy_from_slice(&local_x.to_be_bytes());
        idx += 4;
        wire_buf[idx..idx + 4].copy_from_slice(&local_y.to_be_bytes());
        idx += 4;
        wire_buf[idx..idx + 4].copy_from_slice(&local_z.to_be_bytes());
        idx += 4;
        wire_buf[idx..idx + 4].copy_from_slice(&yaw_degrees.to_be_bytes());
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

    /// Copies visible entity IDs into `out` slice without dynamic allocation.
    ///
    /// Returns the number of entity IDs written to `out`.
    pub fn get_visible_entities_into(&self, out: &mut [u32]) -> usize {
        self.world_view.get_visible_entities_into(out)
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
                let item_id = u32::from_be_bytes([payload[5], payload[6], payload[7], payload[8]]);
                let amount =
                    u32::from_be_bytes([payload[9], payload[10], payload[11], payload[12]]);

                events.push(ClientEvent::LootAcquired {
                    entity_id,
                    item_id,
                    amount,
                });
            }
            // Cast started: type (1B) + entity_id (4B) + ability_id (4B) + duration_ticks (4B) = 13B
            3 if payload.len() >= 13 => {
                let entity_id =
                    u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
                let ability_id =
                    u32::from_be_bytes([payload[5], payload[6], payload[7], payload[8]]);
                let duration_ticks =
                    u32::from_be_bytes([payload[9], payload[10], payload[11], payload[12]]);

                events.push(ClientEvent::CastStarted {
                    entity_id,
                    ability_id,
                    duration_ticks,
                });
            }
            // Cast interrupted: type (1B) + entity_id (4B) + ability_id (4B) + reason (1B) = 10B
            4 if payload.len() >= 10 => {
                let entity_id =
                    u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
                let ability_id =
                    u32::from_be_bytes([payload[5], payload[6], payload[7], payload[8]]);
                let reason = payload[9];

                events.push(ClientEvent::CastInterrupted {
                    entity_id,
                    ability_id,
                    reason,
                });
            }
            // Cast completed: type (1B) + entity_id (4B) + ability_id (4B) = 9B
            5 if payload.len() >= 9 => {
                let entity_id =
                    u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
                let ability_id =
                    u32::from_be_bytes([payload[5], payload[6], payload[7], payload[8]]);

                events.push(ClientEvent::CastCompleted {
                    entity_id,
                    ability_id,
                });
            }
            // Chat received: type (1B) + channel (1B) + sender_id (4B) + text_len (2B) + text_bytes = 8B + len
            6 if payload.len() >= 8 => {
                let channel = payload[1];
                let sender_id =
                    u32::from_be_bytes([payload[2], payload[3], payload[4], payload[5]]);
                let text_len = u16::from_be_bytes([payload[6], payload[7]]) as usize;
                if payload.len() >= 8 + text_len {
                    let message = String::from_utf8_lossy(&payload[8..8 + text_len]).into_owned();
                    events.push(ClientEvent::ChatMessageReceived {
                        channel,
                        sender_id,
                        message,
                    });
                }
            }
            // Equipment changed: type (1B) + entity_id (4B) + slot (1B) + item_id (4B) = 10B
            7 if payload.len() >= 10 => {
                let entity_id =
                    u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
                let slot = payload[5];
                let item_id = u32::from_be_bytes([payload[6], payload[7], payload[8], payload[9]]);

                events.push(ClientEvent::EquipmentChanged {
                    entity_id,
                    slot,
                    item_id,
                });
            }
            // Party updated: type (1B) + party_id (8B) + leader_id (8B) + member_count (1B) = 18B
            8 if payload.len() >= 18 => {
                let party_id = u64::from_be_bytes([
                    payload[1], payload[2], payload[3], payload[4], payload[5], payload[6],
                    payload[7], payload[8],
                ]);
                let leader_account_id = u64::from_be_bytes([
                    payload[9],
                    payload[10],
                    payload[11],
                    payload[12],
                    payload[13],
                    payload[14],
                    payload[15],
                    payload[16],
                ]);
                let member_count = payload[17];

                events.push(ClientEvent::PartyUpdated {
                    party_id,
                    leader_account_id,
                    member_count,
                });
            }
            // Time dilation changed: type (1B) + factor_raw (8B) = 9B
            9 if payload.len() >= 9 => {
                let factor_raw = i64::from_be_bytes([
                    payload[1], payload[2], payload[3], payload[4], payload[5], payload[6],
                    payload[7], payload[8],
                ]);
                let time_dilation = (factor_raw as f32) / 4294967296.0;
                events.push(ClientEvent::TimeDilationChanged { time_dilation });
            }
            // State correction (rubber-band): type (1B) + entity_id (4B) + tick (8B) + pos (24B) + yaw (1B) + flags (1B) + vel (24B) = 63B
            10 if payload.len() >= 63 => {
                let entity_id =
                    u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
                let tick = u64::from_be_bytes([
                    payload[5],
                    payload[6],
                    payload[7],
                    payload[8],
                    payload[9],
                    payload[10],
                    payload[11],
                    payload[12],
                ]);
                let pos_x_raw = i64::from_be_bytes([
                    payload[13],
                    payload[14],
                    payload[15],
                    payload[16],
                    payload[17],
                    payload[18],
                    payload[19],
                    payload[20],
                ]);
                let pos_y_raw = i64::from_be_bytes([
                    payload[21],
                    payload[22],
                    payload[23],
                    payload[24],
                    payload[25],
                    payload[26],
                    payload[27],
                    payload[28],
                ]);
                let pos_z_raw = i64::from_be_bytes([
                    payload[29],
                    payload[30],
                    payload[31],
                    payload[32],
                    payload[33],
                    payload[34],
                    payload[35],
                    payload[36],
                ]);
                let yaw_byte = payload[37];
                let flags = payload[38];
                let vel_x_raw = i64::from_be_bytes([
                    payload[39],
                    payload[40],
                    payload[41],
                    payload[42],
                    payload[43],
                    payload[44],
                    payload[45],
                    payload[46],
                ]);
                let vel_y_raw = i64::from_be_bytes([
                    payload[47],
                    payload[48],
                    payload[49],
                    payload[50],
                    payload[51],
                    payload[52],
                    payload[53],
                    payload[54],
                ]);
                let vel_z_raw = i64::from_be_bytes([
                    payload[55],
                    payload[56],
                    payload[57],
                    payload[58],
                    payload[59],
                    payload[60],
                    payload[61],
                    payload[62],
                ]);

                let pos_x = (pos_x_raw as f64 / 4294967296.0) as f32;
                let pos_y = (pos_y_raw as f64 / 4294967296.0) as f32;
                let pos_z = (pos_z_raw as f64 / 4294967296.0) as f32;
                let vel_x = (vel_x_raw as f64 / 4294967296.0) as f32;
                let vel_y = (vel_y_raw as f64 / 4294967296.0) as f32;
                let vel_z = (vel_z_raw as f64 / 4294967296.0) as f32;
                let yaw_deg = QuantizedYaw::from_byte(yaw_byte).to_degrees() as f32;

                events.push(ClientEvent::StateCorrection {
                    entity_id,
                    tick,
                    position: [pos_x, pos_y, pos_z],
                    velocity: [vel_x, vel_y, vel_z],
                    yaw_deg,
                    flags,
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
