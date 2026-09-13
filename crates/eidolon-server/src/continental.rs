//! Continental and planetary cluster orchestrator.
//!
//! Coordinates multi-worker nodes over UDP sockets or in-process threads across massive
//! continental landmasses: executes dynamic adaptive shard rebalancing, 500ms predictive
//! boundary pre-handshakes, and macro HLOD terrain heightfield queries.

use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::global_coord::GlobalCoord;
use eidolon_core::vehicle::VehicleKinematics;
use eidolon_spatial::bvh::RayHit;
use eidolon_spatial::hlod::{HlodGrid, TerrainTile};
use eidolon_world::adaptive_shard::{AdaptiveShardManager, ShardRebalanceAction};
use eidolon_world::predictive_migration::{PredictiveMigrationPreAuth, PredictiveSeamPredictor};

use crate::facade::AppError;
use crate::multi_process::RealSocketPacket;

/// Cluster UDP opcode for predictive migration pre-authorization notices.
pub const OP_PREDICTIVE_MIGRATION_PRE_AUTH: u8 = 10;

/// Cluster UDP opcode for predictive migration final commit handoffs.
pub const OP_PREDICTIVE_MIGRATION_COMMIT: u8 = 11;

/// Cluster UDP opcode for dynamic shard rebalance announcements.
pub const OP_SHARD_REBALANCE: u8 = 12;

/// Cluster UDP opcode for requesting a macro terrain chunk.
pub const OP_TERRAIN_CHUNK_REQUEST: u8 = 13;

/// Cluster UDP opcode for streaming macro terrain elevation data.
pub const OP_TERRAIN_CHUNK_DATA: u8 = 14;

/// Operational metrics for the continental orchestrator.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ContinentalMetrics {
    /// Number of dynamic sector rebalances executed.
    pub rebalances_executed: u64,
    /// Number of predictive pre-authorization tokens issued.
    pub pre_auths_issued: u64,
    /// Number of predictive migrations successfully committed.
    pub pre_auths_committed: u64,
    /// Number of terrain elevation queries performed.
    pub terrain_queries: u64,
    /// Number of ray-terrain line of sight checks performed.
    pub raycasts_executed: u64,
}

/// Continental cluster orchestrator coordinating worker shards and planetary terrain.
#[derive(Debug)]
pub struct ContinentalOrchestrator {
    /// Local worker shard identifier simulated by this instance.
    local_shard_id: u32,
    /// Dynamic adaptive shard manager distributing 256m sectors across workers.
    shard_manager: AdaptiveShardManager,
    /// Predictive seam monitor projecting 500ms trajectories.
    predictor: PredictiveSeamPredictor,
    /// Macro HLOD grid storing 512m terrain tiles.
    hlod_grid: HlodGrid,
    /// UDP socket for inter-worker cluster communication.
    socket: Option<UdpSocket>,
    /// Address map of remote worker shards: shard_id -> SocketAddr.
    node_addrs: HashMap<u32, SocketAddr>,
    /// Active pre-authorization tokens acknowledged from peers: token_id -> PreAuth.
    inbound_pre_auths: HashMap<u64, PredictiveMigrationPreAuth>,
    /// Current server simulation tick.
    current_tick: u64,
    /// Cumulative operational metrics.
    metrics: ContinentalMetrics,
}

impl ContinentalOrchestrator {
    /// Creates a new continental orchestrator for the specified local shard.
    pub fn new(local_shard_id: u32, num_shards: usize) -> Result<Self, AppError> {
        let shard_manager = AdaptiveShardManager::new(num_shards).map_err(AppError::World)?;
        let predictor = PredictiveSeamPredictor::default();
        let hlod_grid = HlodGrid::new();

        Ok(Self {
            local_shard_id,
            shard_manager,
            predictor,
            hlod_grid,
            socket: None,
            node_addrs: HashMap::new(),
            inbound_pre_auths: HashMap::new(),
            current_tick: 0,
            metrics: ContinentalMetrics::default(),
        })
    }

    /// Binds an operating system UDP socket to the specified address.
    pub fn bind_socket<A: ToSocketAddrs>(&mut self, addr: A) -> Result<SocketAddr, AppError> {
        let socket = UdpSocket::bind(addr).map_err(AppError::Io)?;
        socket.set_nonblocking(true).map_err(AppError::Io)?;
        let local_addr = socket.local_addr().map_err(AppError::Io)?;
        self.socket = Some(socket);
        Ok(local_addr)
    }

    /// Registers the network socket address for a remote worker shard.
    pub fn register_node_addr(&mut self, shard_id: u32, addr: SocketAddr) {
        self.node_addrs.insert(shard_id, addr);
    }

    /// Returns the local worker shard identifier.
    pub const fn local_shard_id(&self) -> u32 {
        self.local_shard_id
    }

    /// Returns a reference to the adaptive shard manager.
    pub const fn shard_manager(&self) -> &AdaptiveShardManager {
        &self.shard_manager
    }

    /// Returns a mutable reference to the adaptive shard manager.
    pub fn shard_manager_mut(&mut self) -> &mut AdaptiveShardManager {
        &mut self.shard_manager
    }

    /// Returns a reference to the predictive seam monitor.
    pub const fn predictor(&self) -> &PredictiveSeamPredictor {
        &self.predictor
    }

    /// Returns a mutable reference to the predictive seam monitor.
    pub fn predictor_mut(&mut self) -> &mut PredictiveSeamPredictor {
        &mut self.predictor
    }

    /// Returns a reference to the macro HLOD terrain grid.
    pub const fn hlod_grid(&self) -> &HlodGrid {
        &self.hlod_grid
    }

    /// Returns a mutable reference to the macro HLOD terrain grid.
    pub fn hlod_grid_mut(&mut self) -> &mut HlodGrid {
        &mut self.hlod_grid
    }

    /// Returns operational metrics.
    pub const fn metrics(&self) -> &ContinentalMetrics {
        &self.metrics
    }

    /// Loads a 512m macro terrain tile into the HLOD grid.
    pub fn load_terrain_tile(&mut self, tile: TerrainTile) {
        self.hlod_grid.insert_tile(tile);
    }

    /// Samples ground elevation at a hierarchical global coordinate.
    pub fn sample_terrain_elevation(&mut self, coord: &GlobalCoord) -> Option<Fixed64> {
        self.metrics.terrain_queries += 1;
        let cont = coord.to_continuous();
        self.hlod_grid.sample_elevation(cont.x, cont.z)
    }

    /// Executes a ray-terrain line of sight check from a hierarchical global coordinate.
    pub fn raycast_terrain(
        &mut self,
        origin: &GlobalCoord,
        dir: Vec3Fix,
        max_distance: Fixed64,
    ) -> Option<RayHit> {
        self.metrics.raycasts_executed += 1;
        let cont_origin = origin.to_continuous();
        self.hlod_grid.raycast(cont_origin, dir, max_distance)
    }

    /// Evaluates entity movement for predictive boundary crossings and issues pre-auth tokens.
    pub fn evaluate_entity_movement(
        &mut self,
        entity_id: u32,
        coord: GlobalCoord,
        velocity: Vec3Fix,
    ) -> Option<PredictiveMigrationPreAuth> {
        let token = self.predictor.evaluate_and_pre_authenticate(
            entity_id,
            coord,
            velocity,
            self.local_shard_id,
            &self.shard_manager,
            self.current_tick,
        )?;

        self.metrics.pre_auths_issued += 1;

        // Send pre-auth replication message to target node over UDP socket if remote
        if let Some(target_addr) = self.node_addrs.get(&token.target_shard) {
            let mut payload = Vec::with_capacity(32);
            payload.extend_from_slice(&token.token_id.to_be_bytes());
            payload.extend_from_slice(&token.entity_id.to_be_bytes());
            payload.extend_from_slice(&token.source_shard.to_be_bytes());
            payload.extend_from_slice(&token.target_shard.to_be_bytes());
            payload.extend_from_slice(&token.target_sector_x.to_be_bytes());
            payload.extend_from_slice(&token.target_sector_z.to_be_bytes());
            payload.extend_from_slice(&token.issued_tick.to_be_bytes());
            payload.extend_from_slice(&token.expiry_tick.to_be_bytes());

            let packet = RealSocketPacket {
                opcode: OP_PREDICTIVE_MIGRATION_PRE_AUTH,
                identifier: entity_id as u64,
                sequence: token.token_id,
                payload,
            };

            let mut send_buf = [0u8; 256];
            if let Ok(encoded_len) = packet.encode(&mut send_buf) {
                if let Some(sock) = &self.socket {
                    let _ = sock.send_to(&send_buf[..encoded_len], target_addr);
                }
            }
        }

        Some(token)
    }

    /// Evaluates high-speed vehicle kinematics for predictive boundary crossings.
    pub fn evaluate_vehicle_movement(
        &mut self,
        entity_id: u32,
        coord: GlobalCoord,
        vehicle: &VehicleKinematics,
    ) -> Option<PredictiveMigrationPreAuth> {
        self.evaluate_entity_movement(entity_id, coord, vehicle.velocity)
    }

    /// Commits a pre-authenticated boundary migration when an entity physically crosses the seam.
    pub fn commit_migration(
        &mut self,
        token_id: u64,
        entity_id: u32,
        target_shard: u32,
    ) -> Result<PredictiveMigrationPreAuth, AppError> {
        let token = self
            .predictor
            .validate_and_consume(token_id, entity_id, target_shard, self.current_tick)
            .map_err(AppError::World)?;

        self.metrics.pre_auths_committed += 1;

        // Notify target worker node of commit
        if let Some(target_addr) = self.node_addrs.get(&target_shard) {
            let mut payload = Vec::with_capacity(16);
            payload.extend_from_slice(&token_id.to_be_bytes());
            payload.extend_from_slice(&entity_id.to_be_bytes());

            let packet = RealSocketPacket {
                opcode: OP_PREDICTIVE_MIGRATION_COMMIT,
                identifier: entity_id as u64,
                sequence: token_id,
                payload,
            };

            let mut send_buf = [0u8; 128];
            if let Ok(encoded_len) = packet.encode(&mut send_buf) {
                if let Some(sock) = &self.socket {
                    let _ = sock.send_to(&send_buf[..encoded_len], target_addr);
                }
            }
        }

        Ok(token)
    }

    /// Advances the simulation tick, purges expired tokens, and evaluates dynamic shard rebalancing.
    pub fn step_tick(
        &mut self,
        current_tick: u64,
        out_rebalances: &mut Vec<ShardRebalanceAction>,
    ) -> Result<(), AppError> {
        self.current_tick = current_tick;
        self.predictor.purge_expired(current_tick);

        let before_count = out_rebalances.len();
        self.shard_manager
            .evaluate_rebalance(current_tick, out_rebalances);
        let new_actions = out_rebalances.len() - before_count;

        if new_actions > 0 {
            self.metrics.rebalances_executed += new_actions as u64;

            // Broadcast rebalance actions over UDP socket to cluster peers
            for action in &out_rebalances[before_count..] {
                let mut payload = Vec::with_capacity(16);
                payload.extend_from_slice(&action.sector_x.to_be_bytes());
                payload.extend_from_slice(&action.sector_z.to_be_bytes());
                payload.extend_from_slice(&action.from_shard.to_be_bytes());
                payload.extend_from_slice(&action.to_shard.to_be_bytes());

                let packet = RealSocketPacket {
                    opcode: OP_SHARD_REBALANCE,
                    identifier: action.to_shard as u64,
                    sequence: current_tick,
                    payload,
                };

                let mut send_buf = [0u8; 128];
                if let Ok(encoded_len) = packet.encode(&mut send_buf) {
                    if let Some(sock) = &self.socket {
                        for addr in self.node_addrs.values() {
                            let _ = sock.send_to(&send_buf[..encoded_len], addr);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Dispatches an incoming cluster packet received from a peer node.
    pub fn handle_cluster_packet(
        &mut self,
        packet: &RealSocketPacket,
        _peer_addr: SocketAddr,
    ) -> Result<(), AppError> {
        match packet.opcode {
            OP_PREDICTIVE_MIGRATION_PRE_AUTH if packet.payload.len() >= 36 => {
                let token_id = u64::from_be_bytes([
                    packet.payload[0],
                    packet.payload[1],
                    packet.payload[2],
                    packet.payload[3],
                    packet.payload[4],
                    packet.payload[5],
                    packet.payload[6],
                    packet.payload[7],
                ]);
                let entity_id = u32::from_be_bytes([
                    packet.payload[8],
                    packet.payload[9],
                    packet.payload[10],
                    packet.payload[11],
                ]);
                let source_shard = u32::from_be_bytes([
                    packet.payload[12],
                    packet.payload[13],
                    packet.payload[14],
                    packet.payload[15],
                ]);
                let target_shard = u32::from_be_bytes([
                    packet.payload[16],
                    packet.payload[17],
                    packet.payload[18],
                    packet.payload[19],
                ]);
                let target_sector_x = i32::from_be_bytes([
                    packet.payload[20],
                    packet.payload[21],
                    packet.payload[22],
                    packet.payload[23],
                ]);
                let target_sector_z = i32::from_be_bytes([
                    packet.payload[24],
                    packet.payload[25],
                    packet.payload[26],
                    packet.payload[27],
                ]);
                let issued_tick = u64::from_be_bytes([
                    packet.payload[28],
                    packet.payload[29],
                    packet.payload[30],
                    packet.payload[31],
                    packet.payload[32],
                    packet.payload[33],
                    packet.payload[34],
                    packet.payload[35],
                ]);

                let pre_auth = PredictiveMigrationPreAuth {
                    token_id,
                    entity_id,
                    source_shard,
                    target_shard,
                    target_sector_x,
                    target_sector_z,
                    issued_tick,
                    expiry_tick: issued_tick + 30,
                    is_consumed: false,
                };

                self.inbound_pre_auths.insert(token_id, pre_auth);
            }
            OP_PREDICTIVE_MIGRATION_COMMIT if packet.payload.len() >= 8 => {
                let token_id = u64::from_be_bytes([
                    packet.payload[0],
                    packet.payload[1],
                    packet.payload[2],
                    packet.payload[3],
                    packet.payload[4],
                    packet.payload[5],
                    packet.payload[6],
                    packet.payload[7],
                ]);
                if let Some(token) = self.inbound_pre_auths.get_mut(&token_id) {
                    token.is_consumed = true;
                    self.metrics.pre_auths_committed += 1;
                }
            }
            OP_SHARD_REBALANCE if packet.payload.len() >= 16 => {
                let sx = i32::from_be_bytes([
                    packet.payload[0],
                    packet.payload[1],
                    packet.payload[2],
                    packet.payload[3],
                ]);
                let sz = i32::from_be_bytes([
                    packet.payload[4],
                    packet.payload[5],
                    packet.payload[6],
                    packet.payload[7],
                ]);
                let _from = u32::from_be_bytes([
                    packet.payload[8],
                    packet.payload[9],
                    packet.payload[10],
                    packet.payload[11],
                ]);
                let to = u32::from_be_bytes([
                    packet.payload[12],
                    packet.payload[13],
                    packet.payload[14],
                    packet.payload[15],
                ]);
                let _ = self.shard_manager.assign_sector(sx, sz, to);
            }
            _ => {}
        }

        Ok(())
    }

    /// Polls the UDP socket for incoming cluster messages and processes them.
    pub fn poll_incoming_packets(&mut self) -> Result<usize, AppError> {
        let mut packets = Vec::new();
        if let Some(sock) = &self.socket {
            let mut buf = [0u8; 1024];
            loop {
                match sock.recv_from(&mut buf) {
                    Ok((len, peer_addr)) => {
                        if let Ok(packet) = RealSocketPacket::decode(&buf[..len]) {
                            packets.push((packet, peer_addr));
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(AppError::Io(e)),
                }
            }
        }

        let count = packets.len();
        for (packet, peer_addr) in packets {
            self.handle_cluster_packet(&packet, peer_addr)?;
        }

        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eidolon_core::quant::QuantizedYaw;
    use eidolon_core::vehicle::VehicleType;
    use eidolon_spatial::hlod::MacroTileCoord;

    #[test]
    fn test_continental_orchestrator_initialization() {
        let orch = ContinentalOrchestrator::new(0, 4);
        assert!(orch.is_ok());
        let o = orch.unwrap();
        assert_eq!(o.local_shard_id(), 0);
        assert_eq!(o.metrics().rebalances_executed, 0);
    }

    #[test]
    fn test_terrain_elevation_and_raycast() {
        let mut orch = ContinentalOrchestrator::new(0, 4).unwrap();
        // Load flat terrain tile at tile (0, 0) with elevation 25.0m
        let tile = TerrainTile::new_flat(
            MacroTileCoord {
                tile_x: 0,
                tile_z: 0,
            },
            Fixed64::from_i32(25),
        );
        orch.load_terrain_tile(tile);

        // Global coordinate at Sector (0, 0), local (100m, 0m, 100m)
        let coord = GlobalCoord::from_sector_and_local(0, 0, 100.0, 0.0, 100.0);
        let elev = orch.sample_terrain_elevation(&coord);
        assert!(elev.is_some());
        assert_eq!(elev.unwrap().to_i32(), 25);

        // Raycast downward from height 50m
        let ray_origin = GlobalCoord::from_sector_and_local(0, 0, 100.0, 50.0, 100.0);
        let ray_dir = Vec3Fix::from_f64(0.0, -1.0, 0.0);
        let hit = orch.raycast_terrain(&ray_origin, ray_dir, Fixed64::from_i32(100));
        assert!(hit.is_some());
        let h = hit.unwrap();
        assert_eq!(h.hit_point.y.to_i32(), 25);
        assert_eq!(h.distance.to_i32(), 25);
    }

    #[test]
    fn test_predictive_pre_handshake_flow() {
        let mut orch = ContinentalOrchestrator::new(0, 4).unwrap();
        orch.shard_manager_mut().assign_sector(0, 0, 0).unwrap();
        orch.shard_manager_mut().assign_sector(1, 0, 1).unwrap();

        // Speeding vehicle traveling at 150 m/s towards sector 1
        let mut vehicle = VehicleKinematics::new(
            VehicleType::Aircraft,
            Vec3Fix::from_f64(230.0, 0.0, 50.0),
            QuantizedYaw::from_degrees(90.0),
        );
        vehicle.velocity = Vec3Fix::from_f64(150.0, 0.0, 0.0);

        let coord = GlobalCoord::from_sector_and_local(0, 0, 230.0, 0.0, 50.0);
        let pre_auth = orch.evaluate_vehicle_movement(123, coord, &vehicle);
        assert!(pre_auth.is_some());
        let token = pre_auth.unwrap();
        assert_eq!(token.entity_id, 123);
        assert_eq!(token.target_shard, 1);

        // Commit migration
        let commit_res = orch.commit_migration(token.token_id, 123, 1);
        assert!(commit_res.is_ok());
        assert_eq!(orch.metrics().pre_auths_committed, 1);
    }
}
