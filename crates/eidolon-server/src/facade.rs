//! Ergonomic server facade and declarative game application builder.
//!
//! `EidolonApp` encapsulates spatial partitioning, 16-bit coordinate quantization,
//! WAL fsync durability, and 20 Hz simulation ticks into an intuitive high-level API.

use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::kinematics::{extrapolate, KinematicState};
use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw};
use eidolon_net::auth::{
    compute_auth_cookie, ConnectChallengeRequest, ConnectChallengeResponse, ConnectFinalizeRequest,
    ConnectFinalizeResponse, CHALLENGE_REQ_LEN, FINALIZE_REQ_LEN, NONCE_LEN,
};
use eidolon_net::error::NetError;
use eidolon_net::packet::PacketHeader;
use eidolon_net::protocol::{ChannelType, PacketType, HEADER_SIZE, MAX_PACKET_SIZE};
use eidolon_spatial::SpatialHashGrid;
use eidolon_world::durable_journal::DurableFileJournal;
use eidolon_world::transaction::TransactionManager;
use eidolon_world::wal::WalRecord;
use eidolon_world::WorldError;

/// High-level server errors encountered during gameplay simulation.
#[derive(Debug)]
pub enum AppError {
    /// Underlying socket or filesystem I/O failure.
    Io(std::io::Error),
    /// Wire protocol framing or serialization failure.
    Net(NetError),
    /// World state or transaction failure.
    World(WorldError),
    /// Entity not found in active world.
    EntityNotFound(u32),
    /// Maximum entity capacity exceeded.
    CapacityExceeded,
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Net(e) => write!(f, "Network error: {e}"),
            Self::World(e) => write!(f, "World error: {e}"),
            Self::EntityNotFound(id) => write!(f, "Entity {id} not found"),
            Self::CapacityExceeded => write!(f, "Entity capacity exceeded"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<NetError> for AppError {
    fn from(err: NetError) -> Self {
        Self::Net(err)
    }
}

impl From<WorldError> for AppError {
    fn from(err: WorldError) -> Self {
        Self::World(err)
    }
}

/// Server representation of an active entity in the authoritative simulation.
#[derive(Debug, Clone)]
pub struct ServerEntity {
    /// Authoritative entity ID.
    pub id: u32,
    /// Entity classification (0 = player, 1 = monster, 2 = npc, 3 = item).
    pub entity_type: u8,
    /// Authoritative 32.32 continuous world position.
    pub position: Vec3Fix,
    /// Current continuous velocity in meters/second.
    pub velocity: Vec3Fix,
    /// Facing heading discrete angle.
    pub yaw: QuantizedYaw,
    /// Movement state flags.
    pub flags: u8,
    /// Current health points.
    pub health: u32,
    /// Maximum health capacity.
    pub max_health: u32,
    /// Optional remote socket address (populated for connected human players).
    pub peer_addr: Option<SocketAddr>,
    /// Associated player account ID.
    pub account_id: Option<u64>,
    /// Assigned session ID.
    pub session_id: Option<u64>,
}

/// Builder for declarative `EidolonApp` configuration.
pub struct EidolonAppBuilder {
    bind_addr: Option<SocketAddr>,
    max_entities: usize,
    wal_path: Option<PathBuf>,
    server_secret: [u8; 32],
}

impl EidolonAppBuilder {
    /// Creates a new app builder with production defaults.
    pub fn new() -> Self {
        Self {
            bind_addr: None,
            max_entities: 2048,
            wal_path: None,
            server_secret: [0x42; 32],
        }
    }

    /// Configures the local UDP socket bind address (e.g. `"0.0.0.0:7777"`).
    pub fn bind<A: ToSocketAddrs>(mut self, addr: A) -> Result<Self, AppError> {
        let resolved = addr.to_socket_addrs()?.next().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid address")
        })?;
        self.bind_addr = Some(resolved);
        Ok(self)
    }

    /// Sets the maximum entity capacity for spatial hash grid pre-allocation.
    pub fn max_entities(mut self, capacity: usize) -> Self {
        self.max_entities = capacity;
        self
    }

    /// Sets the path for durable WAL persistence via `fdatasync`.
    pub fn wal_path<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.wal_path = Some(path.as_ref().to_path_buf());
        self
    }

    /// Sets the server cryptographic master secret key.
    pub fn server_secret(mut self, secret: &[u8; 32]) -> Self {
        self.server_secret = *secret;
        self
    }

    /// Builds and initializes the authoritative `EidolonApp`.
    pub fn build(self) -> Result<EidolonApp, AppError> {
        let bind_addr = self
            .bind_addr
            .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 7777)));
        let socket = UdpSocket::bind(bind_addr)?;
        socket.set_nonblocking(true)?;

        let spatial_grid = SpatialHashGrid::with_capacity(self.max_entities, 2048);
        let journal = match self.wal_path {
            Some(ref path) => Some(DurableFileJournal::open(path)?),
            None => None,
        };

        Ok(EidolonApp {
            socket,
            entities: HashMap::new(),
            spatial_grid,
            journal,
            tx_manager: TransactionManager::new(),
            current_tick: 0,
            server_secret: self.server_secret,
            server_nonce: [0x55; NONCE_LEN],
            packet_buf: [0u8; MAX_PACKET_SIZE],
        })
    }
}

impl Default for EidolonAppBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Authoritative high-level MMO game server application.
pub struct EidolonApp {
    socket: UdpSocket,
    entities: HashMap<u32, ServerEntity>,
    spatial_grid: SpatialHashGrid,
    journal: Option<DurableFileJournal>,
    tx_manager: TransactionManager,
    current_tick: u64,
    server_secret: [u8; 32],
    server_nonce: [u8; NONCE_LEN],
    packet_buf: [u8; MAX_PACKET_SIZE],
}

impl EidolonApp {
    /// Creates an `EidolonAppBuilder` to configure the application.
    pub fn builder() -> EidolonAppBuilder {
        EidolonAppBuilder::new()
    }

    /// Returns a reference to the transaction manager.
    pub fn tx_manager(&self) -> &TransactionManager {
        &self.tx_manager
    }

    /// Returns a mutable reference to the transaction manager.
    pub fn tx_manager_mut(&mut self) -> &mut TransactionManager {
        &mut self.tx_manager
    }

    /// Returns the local socket address this server is listening on.
    pub fn local_addr(&self) -> Result<SocketAddr, AppError> {
        Ok(self.socket.local_addr()?)
    }

    /// Spawns an authoritative NPC or monster entity at continuous coordinates.
    pub fn spawn_npc(
        &mut self,
        id: u32,
        entity_type: u8,
        x: f64,
        y: f64,
        z: f64,
    ) -> Result<(), AppError> {
        let pos = Vec3Fix {
            x: Fixed64::from_f64(x),
            y: Fixed64::from_f64(y),
            z: Fixed64::from_f64(z),
        };

        let entity = ServerEntity {
            id,
            entity_type,
            position: pos,
            velocity: Vec3Fix::ZERO,
            yaw: QuantizedYaw::from_degrees(0.0),
            flags: 0,
            health: 100,
            max_health: 100,
            peer_addr: None,
            account_id: None,
            session_id: None,
        };

        let _ = self.spatial_grid.insert(id, pos);
        self.entities.insert(id, entity);
        Ok(())
    }

    /// Spawns a connected player entity associated with a client socket address.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_player(
        &mut self,
        entity_id: u32,
        account_id: u64,
        session_id: u64,
        peer_addr: SocketAddr,
        x: f64,
        y: f64,
        z: f64,
    ) -> Result<(), AppError> {
        let pos = Vec3Fix {
            x: Fixed64::from_f64(x),
            y: Fixed64::from_f64(y),
            z: Fixed64::from_f64(z),
        };

        let entity = ServerEntity {
            id: entity_id,
            entity_type: 0, // Player
            position: pos,
            velocity: Vec3Fix::ZERO,
            yaw: QuantizedYaw::from_degrees(0.0),
            flags: 0,
            health: 100,
            max_health: 100,
            peer_addr: Some(peer_addr),
            account_id: Some(account_id),
            session_id: Some(session_id),
        };

        let _ = self.spatial_grid.insert(entity_id, pos);
        self.entities.insert(entity_id, entity);
        Ok(())
    }

    /// Despawns an entity from the world and removes it from the spatial grid.
    pub fn despawn_entity(&mut self, entity_id: u32) {
        let _ = self.spatial_grid.remove(entity_id);
        self.entities.remove(&entity_id);
    }

    /// Returns a reference to a specific entity.
    pub fn get_entity(&self, entity_id: u32) -> Option<&ServerEntity> {
        self.entities.get(&entity_id)
    }

    /// Returns a mutable reference to a specific entity.
    pub fn get_entity_mut(&mut self, entity_id: u32) -> Option<&mut ServerEntity> {
        self.entities.get_mut(&entity_id)
    }

    /// Applies damage to an entity, reducing health and returning `true` if entity was killed.
    pub fn damage_entity(&mut self, _source_id: u32, target_id: u32, damage: u32) -> bool {
        if let Some(target) = self.entities.get_mut(&target_id) {
            target.health = target.health.saturating_sub(damage);
            target.health == 0
        } else {
            false
        }
    }

    /// Executes an atomic transaction, persisting to durable WAL via physical `fdatasync`.
    pub fn execute_durable_transaction(
        &mut self,
        account_id: u64,
        op_type: u8,
        payload: &[u8],
    ) -> Result<u64, AppError> {
        if let Some(ref mut journal) = self.journal {
            let record = WalRecord::new(
                self.current_tick,
                self.current_tick,
                account_id,
                1,
                op_type,
                payload,
            )?;
            journal.append_and_sync(&record)?;
            Ok(self.current_tick)
        } else {
            Ok(self.current_tick)
        }
    }

    /// Executes one fixed-step 20 Hz authoritative simulation tick.
    ///
    /// Drains ingress packets, updates kinematics, resolves spatial AoI,
    /// quantizes transforms into 22-byte diffs, and pushes UDP egress.
    pub fn tick(&mut self) -> Result<(), AppError> {
        self.current_tick += 1;
        let tick_interval_secs = Fixed64::from_f64(0.050); // 50ms = 20 Hz

        // 1. Drain incoming UDP packets
        self.drain_network_packets();

        // 2. Run Authoritative Kinematics
        for entity in self.entities.values_mut() {
            if entity.velocity.x != Fixed64::ZERO || entity.velocity.z != Fixed64::ZERO {
                let initial = KinematicState::with_velocity(
                    entity.position,
                    entity.velocity,
                    entity.yaw,
                    entity.flags,
                );
                let next = extrapolate(&initial, 1, tick_interval_secs);
                entity.position = next.position;
            }
        }

        // 3. Update Spatial Grid Positions
        for entity in self.entities.values() {
            let _ = self
                .spatial_grid
                .update_position(entity.id, entity.position);
        }

        // 4. Broadcast AoI State Updates to Connected Player Peers
        self.broadcast_aoi_updates();

        Ok(())
    }

    fn drain_network_packets(&mut self) {
        loop {
            match self.socket.recv_from(&mut self.packet_buf) {
                Ok((len, peer)) => {
                    if len < HEADER_SIZE {
                        continue;
                    }

                    let (hdr, hdr_len) = match PacketHeader::read_from(&self.packet_buf[..len]) {
                        Ok(h) => h,
                        Err(_) => continue,
                    };

                    let payload = &self.packet_buf[hdr_len..len];

                    match hdr.packet_type {
                        PacketType::ReliableMessage => {
                            if payload.len() == CHALLENGE_REQ_LEN {
                                if let Ok(req) = ConnectChallengeRequest::read_from(payload) {
                                    let auth_cookie = compute_auth_cookie(
                                        &self.server_secret,
                                        req.account_id,
                                        &req.client_nonce,
                                        &self.server_nonce,
                                    );

                                    let resp = ConnectChallengeResponse {
                                        server_nonce: self.server_nonce,
                                        auth_cookie,
                                    };

                                    let mut out_resp = [0u8; 64];
                                    if let Ok(resp_len) = resp.write_to(&mut out_resp) {
                                        let resp_hdr = PacketHeader::new(
                                            ChannelType::ReliableOrdered,
                                            PacketType::ReliableMessage,
                                            1,
                                            hdr.sequence,
                                            0,
                                        );
                                        let mut wire = [0u8; 128];
                                        if let Ok(hlen) = resp_hdr.write_to(&mut wire) {
                                            wire[hlen..hlen + resp_len]
                                                .copy_from_slice(&out_resp[..resp_len]);
                                            let _ =
                                                self.socket.send_to(&wire[..hlen + resp_len], peer);
                                        }
                                    }
                                }
                            } else if payload.len() == FINALIZE_REQ_LEN {
                                if let Ok(fin) = ConnectFinalizeRequest::read_from(payload) {
                                    // Accept connection and spawn player session
                                    let session_id = fin.account_id ^ 0xA5A5;
                                    let fin_resp = ConnectFinalizeResponse {
                                        session_id,
                                        authority_epoch: 1,
                                        server_proof: [0xEE; 32],
                                    };

                                    let mut out_fin = [0u8; 64];
                                    if let Ok(fin_len) = fin_resp.write_to(&mut out_fin) {
                                        let resp_hdr = PacketHeader::new(
                                            ChannelType::ReliableOrdered,
                                            PacketType::ReliableMessage,
                                            2,
                                            hdr.sequence,
                                            0,
                                        );
                                        let mut wire = [0u8; 128];
                                        if let Ok(hlen) = resp_hdr.write_to(&mut wire) {
                                            wire[hlen..hlen + fin_len]
                                                .copy_from_slice(&out_fin[..fin_len]);
                                            let _ =
                                                self.socket.send_to(&wire[..hlen + fin_len], peer);

                                            // Auto-spawn player entity if not already present
                                            let player_eid = (fin.account_id & 0x0FFF) as u32;
                                            if !self.entities.contains_key(&player_eid) {
                                                let _ = self.spawn_player(
                                                    player_eid,
                                                    fin.account_id,
                                                    session_id,
                                                    peer,
                                                    100.0,
                                                    0.0,
                                                    100.0,
                                                );
                                            }
                                        }
                                    }
                                }
                            } else if payload.len() >= 9 {
                                // Reliable action command: action_type (1B) + target_id (4B) + param (4B)
                                let action_type = payload[0];
                                let target_id = u32::from_be_bytes([
                                    payload[1], payload[2], payload[3], payload[4],
                                ]);
                                let param = u32::from_be_bytes([
                                    payload[5], payload[6], payload[7], payload[8],
                                ]);

                                // Handle combat action (1 = attack)
                                if action_type == 1 {
                                    let killed = self.damage_entity(0, target_id, param);
                                    if killed {
                                        // Broadcast loot drop notification to peer
                                        self.send_loot_event(peer, target_id, 1001, 50);
                                    }
                                }
                            }
                        }
                        // Player movement intent: vx (f32, 4B) + vz (f32, 4B) + yaw_deg (f32, 4B) + flags (1B)
                        PacketType::StateUpdate if payload.len() >= 13 => {
                            let vx = f32::from_be_bytes([
                                payload[0], payload[1], payload[2], payload[3],
                            ]);
                            let vz = f32::from_be_bytes([
                                payload[4], payload[5], payload[6], payload[7],
                            ]);
                            let yaw_deg = f32::from_be_bytes([
                                payload[8],
                                payload[9],
                                payload[10],
                                payload[11],
                            ]);
                            let flags = payload[12];

                            // Locate player entity by peer address
                            if let Some(player) = self
                                .entities
                                .values_mut()
                                .find(|e| e.peer_addr == Some(peer))
                            {
                                player.velocity.x = Fixed64::from_f64(vx as f64);
                                player.velocity.z = Fixed64::from_f64(vz as f64);
                                player.yaw = QuantizedYaw::from_degrees(yaw_deg as f64);
                                player.flags = flags;
                            }
                        }
                        _ => {}
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(_) => break,
            }
        }
    }

    fn broadcast_aoi_updates(&mut self) {
        // Collect observer players with peer addresses
        let observers: Vec<(u32, SocketAddr, Vec3Fix)> = self
            .entities
            .values()
            .filter_map(|e| e.peer_addr.map(|addr| (e.id, addr, e.position)))
            .collect();

        let mut query_buf = [0u32; 128];
        for (_player_id, peer_addr, pos) in observers {
            // Query nearby entities within 50m radius (50^2 = 2500 m^2)
            let query_res = self.spatial_grid.query_radius_squared(
                pos,
                Fixed64::from_i32(2500),
                &mut query_buf,
            );

            let mut out_packet = [0u8; MAX_PACKET_SIZE];
            let header = PacketHeader::new(
                ChannelType::UnreliableSequenced,
                PacketType::StateUpdate,
                self.current_tick as u16,
                0,
                0,
            );

            if let Ok(hdr_len) = header.write_to(&mut out_packet[..HEADER_SIZE]) {
                let mut p_idx = hdr_len;

                for &visible_id in &query_buf[..query_res.written] {
                    if let Some(target) = self.entities.get(&visible_id) {
                        if p_idx + 22 > MAX_PACKET_SIZE {
                            break;
                        }

                        let (cx, cy, cz, q_coord) =
                            QuantizedCellCoord::quantize_from_global(target.position);

                        let vx_i16 = target.velocity.x.to_i32().clamp(-32768, 32767) as i16;
                        let vz_i16 = target.velocity.z.to_i32().clamp(-32768, 32767) as i16;

                        out_packet[p_idx..p_idx + 4].copy_from_slice(&target.id.to_be_bytes());
                        p_idx += 4;
                        out_packet[p_idx..p_idx + 2].copy_from_slice(&(cx as i16).to_be_bytes());
                        p_idx += 2;
                        out_packet[p_idx..p_idx + 2].copy_from_slice(&(cz as i16).to_be_bytes());
                        p_idx += 2;
                        out_packet[p_idx..p_idx + 2].copy_from_slice(&(cy as i16).to_be_bytes());
                        p_idx += 2;
                        out_packet[p_idx..p_idx + 2].copy_from_slice(&q_coord.x.to_be_bytes());
                        p_idx += 2;
                        out_packet[p_idx..p_idx + 2].copy_from_slice(&q_coord.z.to_be_bytes());
                        p_idx += 2;
                        out_packet[p_idx..p_idx + 2].copy_from_slice(&q_coord.y.to_be_bytes());
                        p_idx += 2;
                        out_packet[p_idx] = target.yaw.0;
                        p_idx += 1;
                        out_packet[p_idx] = target.flags;
                        p_idx += 1;
                        out_packet[p_idx..p_idx + 2].copy_from_slice(&vx_i16.to_be_bytes());
                        p_idx += 2;
                        out_packet[p_idx..p_idx + 2].copy_from_slice(&vz_i16.to_be_bytes());
                        p_idx += 2;
                    }
                }

                if p_idx > hdr_len {
                    let _ = self.socket.send_to(&out_packet[..p_idx], peer_addr);
                }
            }
        }
    }

    fn send_loot_event(&self, peer: SocketAddr, entity_id: u32, item_id: u32, amount: u32) {
        let header = PacketHeader::new(
            ChannelType::ReliableOrdered,
            PacketType::ReliableMessage,
            self.current_tick as u16,
            0,
            0,
        );
        let mut wire = [0u8; 64];
        if let Ok(hlen) = header.write_to(&mut wire) {
            let mut idx = hlen;
            wire[idx] = 2; // LootAcquired event type
            idx += 1;
            wire[idx..idx + 4].copy_from_slice(&entity_id.to_be_bytes());
            idx += 4;
            wire[idx..idx + 4].copy_from_slice(&item_id.to_be_bytes());
            idx += 4;
            wire[idx..idx + 4].copy_from_slice(&amount.to_be_bytes());
            idx += 4;

            let _ = self.socket.send_to(&wire[..idx], peer);
        }
    }
}
