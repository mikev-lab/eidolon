//! Ergonomic server facade and declarative game application builder.
//!
//! `EidolonApp` encapsulates spatial partitioning, 16-bit coordinate quantization,
//! WAL fsync durability, and 20 Hz simulation ticks into an intuitive high-level API.

use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};

use crate::continental::ContinentalOrchestrator;
use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::geom::SpatialGeometry;
use eidolon_core::global_coord::GlobalCoord;
use eidolon_core::item::EquipmentSlot;
use eidolon_core::kinematics::{extrapolate, KinematicState};
use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw};
use eidolon_core::structure::{InteriorItemRecord, MaterialType, PieceType, SnapSocket};
use eidolon_core::vehicle::VehicleKinematics;
use eidolon_net::auth::{
    compute_auth_cookie, ConnectChallengeRequest, ConnectChallengeResponse, ConnectFinalizeRequest,
    ConnectFinalizeResponse, CHALLENGE_REQ_LEN, FINALIZE_REQ_LEN, NONCE_LEN,
};
use eidolon_net::error::NetError;
use eidolon_net::governor::{BandwidthGovernor, DensityProfile};
use eidolon_net::packet::PacketHeader;
use eidolon_net::protocol::{ChannelType, PacketType, HEADER_SIZE, MAX_PACKET_SIZE};
use eidolon_spatial::bvh::RayHit;
use eidolon_spatial::hlod::TerrainTile;
use eidolon_spatial::tier::FrequencyTier;
use eidolon_spatial::SpatialHashGrid;
use eidolon_world::ability::{get_ability_definition, AbilityShape, CastState, CooldownTracker};
use eidolon_world::building::StructureManager;
use eidolon_world::chat::{ChatChannel, ChatRateLimiter};
use eidolon_world::chunk_manifest::ChunkManifestManager;
use eidolon_world::durable_journal::DurableFileJournal;
use eidolon_world::equipment::EquipmentContainer;
use eidolon_world::interior::InteriorCellManager;
use eidolon_world::npc_ecosystem::NpcEcosystemManager;
use eidolon_world::party::{PartyManager, PartyMember};
use eidolon_world::predictive_migration::PredictiveMigrationPreAuth;
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
    /// Current mana resource points.
    pub mana: u32,
    /// Maximum mana capacity.
    pub max_mana: u32,
    /// Active ability cast progress state.
    pub cast_state: CastState,
    /// Character ability cooldown tracker.
    pub cooldowns: CooldownTracker,
    /// Equipped character gear across all 9 slots.
    pub equipment: EquipmentContainer,
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
    density_profile: DensityProfile,
}

impl EidolonAppBuilder {
    /// Creates a new app builder with production defaults.
    pub fn new() -> Self {
        Self {
            bind_addr: None,
            max_entities: 2048,
            wal_path: None,
            server_secret: [0x42; 32],
            density_profile: DensityProfile::StandardMMO,
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

    /// Sets the network density profile and per-client bandwidth budget.
    pub fn density_profile(mut self, profile: DensityProfile) -> Self {
        self.density_profile = profile;
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

        let bandwidth_governor = BandwidthGovernor::new(self.max_entities, self.density_profile);

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
            chat_limiter: ChatRateLimiter::new(10, 20),
            party_manager: PartyManager::new(),
            density_profile: self.density_profile,
            bandwidth_governor,
            time_dilation: Fixed64::ONE,
            structure_manager: StructureManager::new(),
            interior_manager: InteriorCellManager::new(),
            chunk_manifest_manager: ChunkManifestManager::new(),
            continental_orchestrator: None,
            npc_ecosystem: NpcEcosystemManager::new(),
            player_coords_scratch: Vec::with_capacity(self.max_entities),
            player_info_scratch: Vec::with_capacity(self.max_entities),
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
    chat_limiter: ChatRateLimiter,
    party_manager: PartyManager,
    density_profile: DensityProfile,
    bandwidth_governor: BandwidthGovernor,
    time_dilation: Fixed64,
    structure_manager: StructureManager,
    interior_manager: InteriorCellManager,
    chunk_manifest_manager: ChunkManifestManager,
    continental_orchestrator: Option<ContinentalOrchestrator>,
    npc_ecosystem: NpcEcosystemManager,
    player_coords_scratch: Vec<(f32, f32, f32)>,
    player_info_scratch: Vec<(u64, (f32, f32, f32), u32)>,
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

    /// Returns a reference to the party manager.
    pub fn party_manager(&self) -> &PartyManager {
        &self.party_manager
    }

    /// Returns a mutable reference to the party manager.
    pub fn party_manager_mut(&mut self) -> &mut PartyManager {
        &mut self.party_manager
    }

    /// Returns a reference to the player structure manager.
    pub fn structure_manager(&self) -> &StructureManager {
        &self.structure_manager
    }

    /// Returns a mutable reference to the player structure manager.
    pub fn structure_manager_mut(&mut self) -> &mut StructureManager {
        &mut self.structure_manager
    }

    /// Returns a reference to the interior cell manager.
    pub fn interior_manager(&self) -> &InteriorCellManager {
        &self.interior_manager
    }

    /// Returns a mutable reference to the interior cell manager.
    pub fn interior_manager_mut(&mut self) -> &mut InteriorCellManager {
        &mut self.interior_manager
    }

    /// Returns a reference to the chunk manifest manager.
    pub fn chunk_manifest_manager(&self) -> &ChunkManifestManager {
        &self.chunk_manifest_manager
    }

    /// Returns a mutable reference to the chunk manifest manager.
    pub fn chunk_manifest_manager_mut(&mut self) -> &mut ChunkManifestManager {
        &mut self.chunk_manifest_manager
    }

    /// Returns a reference to the autonomous NPC ecosystem manager.
    pub fn npc_ecosystem(&self) -> &NpcEcosystemManager {
        &self.npc_ecosystem
    }

    /// Returns a mutable reference to the autonomous NPC ecosystem manager.
    pub fn npc_ecosystem_mut(&mut self) -> &mut NpcEcosystemManager {
        &mut self.npc_ecosystem
    }

    /// Places a modular prefab structure (e.g. SWG house, harvester) in the world.
    pub fn place_modular_prefab(
        &mut self,
        owner_account_id: u64,
        prefab_type_id: u32,
        position: Vec3Fix,
        yaw: QuantizedYaw,
        interior_cell_id: Option<u32>,
    ) -> Result<u32, AppError> {
        let structure_id = self.structure_manager.create_prefab(
            owner_account_id,
            prefab_type_id,
            position,
            yaw,
            interior_cell_id,
        )?;

        self.chunk_manifest_manager
            .register_structure(structure_id, position);

        Ok(structure_id)
    }

    /// Places a new grounded foundation establishing a freeform construction group.
    pub fn place_foundation(
        &mut self,
        owner_account_id: u64,
        position: Vec3Fix,
        yaw: QuantizedYaw,
        material: MaterialType,
    ) -> Result<(u32, u32), AppError> {
        let (structure_id, piece_id) =
            self.structure_manager
                .place_foundation(owner_account_id, position, yaw, material)?;

        self.chunk_manifest_manager
            .register_structure(structure_id, position);

        Ok((structure_id, piece_id))
    }

    /// Snaps and places a child building piece onto an existing parent piece.
    pub fn snap_piece(
        &mut self,
        structure_id: u32,
        parent_piece_id: u32,
        piece_type: PieceType,
        material: MaterialType,
        socket: SnapSocket,
        variant_flags: u8,
    ) -> Result<u32, AppError> {
        let piece_id = self.structure_manager.place_piece(
            structure_id,
            parent_piece_id,
            piece_type,
            material,
            socket,
            variant_flags,
        )?;

        if let Some(structure) = self.structure_manager.get_structure(structure_id) {
            self.chunk_manifest_manager
                .mark_chunk_modified(structure.world_position);
        }

        Ok(piece_id)
    }

    /// Destroys a building piece and triggers cascading collapse of ungrounded pieces.
    pub fn destroy_piece(
        &mut self,
        structure_id: u32,
        piece_id: u32,
    ) -> Result<Vec<u32>, AppError> {
        let collapsed = self
            .structure_manager
            .destroy_piece(structure_id, piece_id)?;

        if let Some(structure) = self.structure_manager.get_structure(structure_id) {
            self.chunk_manifest_manager
                .mark_chunk_modified(structure.world_position);
        }

        Ok(collapsed)
    }

    /// Demolishes a player structure completely and updates spatial chunk manifests.
    pub fn destroy_structure(&mut self, structure_id: u32) -> Result<(), AppError> {
        if let Some(structure) = self.structure_manager.get_structure(structure_id) {
            let _ = self
                .chunk_manifest_manager
                .unregister_structure(structure_id, structure.world_position);
        }
        self.structure_manager.destroy_structure(structure_id)?;
        Ok(())
    }

    /// Performs a raycast against all active player structures in the world.
    pub fn raycast_structures(
        &self,
        origin: Vec3Fix,
        dir: Vec3Fix,
        max_distance: Fixed64,
    ) -> Option<RayHit> {
        self.structure_manager.raycast(origin, dir, max_distance)
    }

    /// Transitions an entity into an interior cell pocket dimension.
    pub fn enter_interior_cell(&mut self, entity_id: u32, cell_id: u32) -> Result<(), AppError> {
        self.interior_manager.enter_cell(entity_id, cell_id)?;
        Ok(())
    }

    /// Transitions an entity out of an interior cell pocket dimension back to open-world space.
    pub fn exit_interior_cell(&mut self, entity_id: u32) -> Result<u32, AppError> {
        let cell_id = self.interior_manager.exit_cell(entity_id)?;
        Ok(cell_id)
    }

    /// Adds a customized decorative item to an interior cell.
    pub fn place_interior_item(
        &mut self,
        cell_id: u32,
        record: InteriorItemRecord,
    ) -> Result<(), AppError> {
        self.interior_manager.place_item(cell_id, record)?;
        Ok(())
    }

    /// Removes a decorative item from an interior cell.
    pub fn remove_interior_item(
        &mut self,
        cell_id: u32,
        item_instance_id: u32,
    ) -> Result<InteriorItemRecord, AppError> {
        let rec = self
            .interior_manager
            .remove_item(cell_id, item_instance_id)?;
        Ok(rec)
    }

    /// Serializes an interior scene manifest for an entity entering a building.
    pub fn build_interior_scene_manifest(
        &self,
        cell_id: u32,
        out_buffer: &mut [u8],
    ) -> Result<usize, AppError> {
        let bytes = self
            .interior_manager
            .build_scene_manifest(cell_id, out_buffer)?;
        Ok(bytes)
    }

    /// Runs decoupled background maintenance decay on player structures.
    pub fn run_structure_maintenance(&mut self, decay_amount: u32) -> usize {
        self.structure_manager
            .run_maintenance_cycle(self.current_tick, decay_amount)
    }

    /// Configures the active network density profile and updates the bandwidth governor.
    pub fn set_density_profile(&mut self, profile: DensityProfile) {
        self.density_profile = profile;
        self.bandwidth_governor.set_profile(profile);
    }

    /// Returns the active network density profile.
    pub fn density_profile(&self) -> DensityProfile {
        self.density_profile
    }

    /// Returns a reference to the bandwidth governor.
    pub fn bandwidth_governor(&self) -> &BandwidthGovernor {
        &self.bandwidth_governor
    }

    /// Returns a mutable reference to the bandwidth governor.
    pub fn bandwidth_governor_mut(&mut self) -> &mut BandwidthGovernor {
        &mut self.bandwidth_governor
    }

    /// Configures the authoritative time dilation factor (clamped to [0.1, 1.0]).
    pub fn set_time_dilation(&mut self, factor: Fixed64) {
        let clamped = factor.clamp(Fixed64::from_f64(0.1), Fixed64::ONE);
        self.time_dilation = clamped;
        self.broadcast_time_dilation(clamped);
    }

    /// Returns the active authoritative time dilation factor.
    pub fn time_dilation(&self) -> Fixed64 {
        self.time_dilation
    }

    fn broadcast_time_dilation(&self, factor: Fixed64) {
        let mut payload = [0u8; 9];
        payload[0] = 9; // TimeDilationChanged
        payload[1..9].copy_from_slice(&factor.raw().to_be_bytes());

        for entity in self.entities.values() {
            if let Some(peer) = entity.peer_addr {
                self.send_reliable_event_to_peer(peer, &payload);
            }
        }
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
            mana: 100,
            max_mana: 100,
            cast_state: CastState::Idle,
            cooldowns: CooldownTracker::new(),
            equipment: EquipmentContainer::new(),
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
            mana: 100,
            max_mana: 100,
            cast_state: CastState::Idle,
            cooldowns: CooldownTracker::new(),
            equipment: EquipmentContainer::new(),
            peer_addr: Some(peer_addr),
            account_id: Some(account_id),
            session_id: Some(session_id),
        };

        let _ = self.spatial_grid.insert(entity_id, pos);
        self.entities.insert(entity_id, entity);
        self.bandwidth_governor.register_client(entity_id);
        Ok(())
    }

    /// Despawns an entity from the world and removes it from the spatial grid.
    pub fn despawn_entity(&mut self, entity_id: u32) {
        let _ = self.spatial_grid.remove(entity_id);
        self.entities.remove(&entity_id);
        self.bandwidth_governor.unregister_client(entity_id);
    }

    /// Returns a reference to all active entities in the world.
    pub fn entities(&self) -> &HashMap<u32, ServerEntity> {
        &self.entities
    }

    /// Returns a reference to a specific entity.
    pub fn get_entity(&self, entity_id: u32) -> Option<&ServerEntity> {
        self.entities.get(&entity_id)
    }

    /// Returns a mutable reference to a specific entity.
    pub fn get_entity_mut(&mut self, entity_id: u32) -> Option<&mut ServerEntity> {
        self.entities.get_mut(&entity_id)
    }

    /// Applies damage to an entity, factoring in attacker attack power and target armor.
    ///
    /// Interrupts active casts on damage and returns `true` if target entity was killed.
    pub fn damage_entity(&mut self, source_id: u32, target_id: u32, raw_damage: u32) -> bool {
        let attack_power_bonus = self
            .entities
            .get(&source_id)
            .map(|s| s.equipment.compute_stats().attack_power)
            .unwrap_or(0);
        let total_damage = raw_damage.saturating_add(attack_power_bonus);

        let mut cast_interrupted = None;

        let killed = if let Some(target) = self.entities.get_mut(&target_id) {
            let armor = target.equipment.compute_stats().armor;
            let effective_damage = total_damage.saturating_sub(armor).max(1);

            // Interrupt any active non-unbreakable cast on damage
            if let CastState::Casting { ability_id, .. } = target.cast_state {
                target.cast_state = CastState::Idle;
                cast_interrupted = Some((target_id, ability_id));
            }

            target.health = target.health.saturating_sub(effective_damage);
            target.health == 0
        } else {
            false
        };

        if let Some((interrupted_target, ability_id)) = cast_interrupted {
            self.broadcast_cast_interrupted(interrupted_target, ability_id, 2); // 2 = Damage
        }

        killed
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
    /// advances cast progress state machines, quantizes transforms, and pushes UDP egress.
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

        // 4. Progress Cast Bars and Check Interrupts
        let current_tick = self.current_tick;
        let mut interrupted_casts = Vec::new();
        let mut completed_casts = Vec::new();

        for entity in self.entities.values_mut() {
            if let CastState::Casting {
                ability_id,
                target_id,
                start_tick,
                duration_ticks,
                start_pos,
            } = entity.cast_state
            {
                // Check movement break threshold (> 0.5m distance from cast origin)
                let diff = entity.position - start_pos;
                let moved_sq = (diff.x * diff.x) + (diff.y * diff.y) + (diff.z * diff.z);
                if moved_sq > Fixed64::from_f64(0.25) {
                    entity.cast_state = CastState::Idle;
                    interrupted_casts.push((entity.id, ability_id, 1u8)); // 1 = Movement
                    continue;
                }

                // Check cast bar completion
                if current_tick >= start_tick + duration_ticks as u64 {
                    entity.cast_state = CastState::Idle;
                    completed_casts.push((entity.id, ability_id, target_id));
                }
            }
        }

        for (caster_id, ability_id, reason) in interrupted_casts {
            self.broadcast_cast_interrupted(caster_id, ability_id, reason);
        }

        for (caster_id, ability_id, target_id) in completed_casts {
            self.resolve_ability_completion(caster_id, ability_id, target_id);
        }

        // 5. Resource regeneration tick (every 20 ticks = 1.0s at 20 Hz)
        if self.current_tick.is_multiple_of(20) {
            for entity in self.entities.values_mut() {
                if entity.mana < entity.max_mana {
                    entity.mana = (entity.mana + 5).min(entity.max_mana);
                }
            }
        }

        // 6. Update Autonomous NPC Ecosystem simulation (LOD-based evaluation)
        self.player_coords_scratch.clear();
        self.player_info_scratch.clear();
        for entity in self.entities.values() {
            if entity.entity_type == 0 {
                let pos = (
                    entity.position.x.to_f32(),
                    entity.position.y.to_f32(),
                    entity.position.z.to_f32(),
                );
                self.player_coords_scratch.push(pos);
                self.player_info_scratch.push((entity.id as u64, pos, 0u32));
            }
        }
        self.npc_ecosystem
            .update_lod_tiers(&self.player_coords_scratch);
        self.npc_ecosystem
            .tick(self.current_tick, 0.050, &self.player_info_scratch);

        // 7. Broadcast AoI State Updates to Connected Player Peers
        self.broadcast_aoi_updates();

        Ok(())
    }

    /// Resolves an ability upon cast bar completion or instant trigger.
    fn resolve_ability_completion(&mut self, caster_id: u32, ability_id: u32, target_id: u32) {
        let (caster_pos, caster_yaw) = match self.entities.get(&caster_id) {
            Some(c) => (c.position, c.yaw),
            None => return,
        };

        let def = match get_ability_definition(ability_id) {
            Some(d) => d,
            None => return,
        };

        // Emit CastCompleted event
        self.broadcast_cast_completed(caster_id, ability_id);

        match def.shape {
            AbilityShape::SingleTarget { max_range } => {
                if let Some(target) = self.entities.get(&target_id) {
                    let diff = target.position - caster_pos;
                    if diff.magnitude_squared() <= max_range * max_range {
                        if def.base_damage > 0 {
                            let killed = self.damage_entity(caster_id, target_id, def.base_damage);
                            self.broadcast_combat_action(caster_id, target_id, 2, def.base_damage); // 2 = Magic
                            if killed {
                                self.broadcast_combat_action(caster_id, target_id, 4, 0); // 4 = Death
                                if let Some(peer) =
                                    self.entities.get(&caster_id).and_then(|c| c.peer_addr)
                                {
                                    self.send_loot_event(peer, target_id, 1001, 50);
                                }
                            }
                        } else if def.base_heal > 0 {
                            if let Some(target_ent) = self.entities.get_mut(&target_id) {
                                target_ent.health =
                                    (target_ent.health + def.base_heal).min(target_ent.max_health);
                            }
                            self.broadcast_combat_action(caster_id, target_id, 3, def.base_heal);
                            // 3 = Heal
                        }
                    }
                }
            }
            AbilityShape::ForwardCone {
                half_angle_deg,
                max_distance,
                max_height,
            } => {
                let mut hits = Vec::new();
                for (id, entity) in &self.entities {
                    if *id != caster_id
                        && SpatialGeometry::test_cone(
                            caster_pos,
                            caster_yaw,
                            half_angle_deg,
                            max_distance,
                            max_height,
                            entity.position,
                        )
                    {
                        hits.push(*id);
                    }
                }

                for tid in hits {
                    if def.base_damage > 0 {
                        let killed = self.damage_entity(caster_id, tid, def.base_damage);
                        self.broadcast_combat_action(caster_id, tid, 2, def.base_damage);
                        if killed {
                            self.broadcast_combat_action(caster_id, tid, 4, 0);
                            if let Some(peer) =
                                self.entities.get(&caster_id).and_then(|c| c.peer_addr)
                            {
                                self.send_loot_event(peer, tid, 1001, 50);
                            }
                        }
                    }
                }
            }
            AbilityShape::RadiusSphere { radius } => {
                let mut hits = Vec::new();
                for (id, entity) in &self.entities {
                    if *id != caster_id
                        && SpatialGeometry::test_sphere(caster_pos, radius, entity.position)
                    {
                        hits.push(*id);
                    }
                }

                for tid in hits {
                    if def.base_damage > 0 {
                        let killed = self.damage_entity(caster_id, tid, def.base_damage);
                        self.broadcast_combat_action(caster_id, tid, 2, def.base_damage);
                        if killed {
                            self.broadcast_combat_action(caster_id, tid, 4, 0);
                            if let Some(peer) =
                                self.entities.get(&caster_id).and_then(|c| c.peer_addr)
                            {
                                self.send_loot_event(peer, tid, 1001, 50);
                            }
                        }
                    }
                }
            }
            AbilityShape::ForwardBox {
                length,
                width,
                height,
            } => {
                let mut hits = Vec::new();
                for (id, entity) in &self.entities {
                    if *id != caster_id
                        && SpatialGeometry::test_box(
                            caster_pos,
                            caster_yaw,
                            length,
                            width,
                            height,
                            entity.position,
                        )
                    {
                        hits.push(*id);
                    }
                }

                for tid in hits {
                    if def.base_damage > 0 {
                        let killed = self.damage_entity(caster_id, tid, def.base_damage);
                        self.broadcast_combat_action(caster_id, tid, 2, def.base_damage);
                        if killed {
                            self.broadcast_combat_action(caster_id, tid, 4, 0);
                        }
                    }
                }
            }
        }
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
                            } else if !payload.is_empty() {
                                let action_type = payload[0];
                                match action_type {
                                    // 1: Melee attack [1, target_id (4B), param (4B)]
                                    1 if payload.len() >= 9 => {
                                        let target_id = u32::from_be_bytes([
                                            payload[1], payload[2], payload[3], payload[4],
                                        ]);
                                        let param = u32::from_be_bytes([
                                            payload[5], payload[6], payload[7], payload[8],
                                        ]);

                                        let source_id = self
                                            .entities
                                            .values()
                                            .find(|e| e.peer_addr == Some(peer))
                                            .map(|e| e.id)
                                            .unwrap_or(0);

                                        let killed =
                                            self.damage_entity(source_id, target_id, param);
                                        self.broadcast_combat_action(
                                            source_id, target_id, 1, param,
                                        );
                                        if killed {
                                            self.broadcast_combat_action(
                                                source_id, target_id, 4, 0,
                                            );
                                            self.send_loot_event(peer, target_id, 1001, 50);
                                        }
                                    }

                                    // 2: Cast ability [2, target_id (4B), ability_id (4B)]
                                    2 if payload.len() >= 9 => {
                                        let target_id = u32::from_be_bytes([
                                            payload[1], payload[2], payload[3], payload[4],
                                        ]);
                                        let ability_id = u32::from_be_bytes([
                                            payload[5], payload[6], payload[7], payload[8],
                                        ]);

                                        if let Some(caster) = self
                                            .entities
                                            .values_mut()
                                            .find(|e| e.peer_addr == Some(peer))
                                        {
                                            let caster_id = caster.id;
                                            let current_tick = self.current_tick;

                                            if let Some(def) = get_ability_definition(ability_id) {
                                                if caster.mana >= def.resource_cost
                                                    && caster
                                                        .cooldowns
                                                        .is_ready(ability_id, current_tick)
                                                {
                                                    caster.mana = caster
                                                        .mana
                                                        .saturating_sub(def.resource_cost);
                                                    caster.cooldowns.trigger(
                                                        ability_id,
                                                        current_tick,
                                                        def.cooldown_ticks,
                                                    );

                                                    if def.cast_duration_ticks == 0 {
                                                        // Instant cast
                                                        self.resolve_ability_completion(
                                                            caster_id, ability_id, target_id,
                                                        );
                                                    } else {
                                                        caster.cast_state = CastState::start_cast(
                                                            ability_id,
                                                            target_id,
                                                            current_tick,
                                                            def.cast_duration_ticks,
                                                            caster.position,
                                                        );
                                                        self.broadcast_cast_started(
                                                            caster_id,
                                                            ability_id,
                                                            def.cast_duration_ticks,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // 3: Equip item [3, inventory_slot (4B), equip_slot (4B)]
                                    3 if payload.len() >= 9 => {
                                        let inv_slot = u32::from_be_bytes([
                                            payload[1], payload[2], payload[3], payload[4],
                                        ]);
                                        let eq_slot_raw = u32::from_be_bytes([
                                            payload[5], payload[6], payload[7], payload[8],
                                        ])
                                            as u8;

                                        if let Some(slot) = EquipmentSlot::from_u8(eq_slot_raw) {
                                            if let Some(player) = self
                                                .entities
                                                .values_mut()
                                                .find(|e| e.peer_addr == Some(peer))
                                            {
                                                let player_id = player.id;
                                                let account_id = player.account_id;

                                                let inv_idx = inv_slot as usize;
                                                let equipped_item = if let Some(aid) = account_id {
                                                    if let Some(account) =
                                                        self.tx_manager.get_account_mut(aid)
                                                    {
                                                        let _ = account.equip_item(inv_idx, slot);
                                                        account
                                                            .equipment
                                                            .get(slot)
                                                            .unwrap_or(inv_slot.max(1))
                                                    } else {
                                                        inv_slot.max(1)
                                                    }
                                                } else {
                                                    inv_slot.max(1)
                                                };

                                                let _ = player.equipment.equip(slot, equipped_item);
                                                self.broadcast_equipment_changed(
                                                    player_id,
                                                    eq_slot_raw,
                                                    equipped_item,
                                                );
                                            }
                                        }
                                    }

                                    // 4: Unequip item [4, equip_slot (4B), 0 (4B)]
                                    4 if payload.len() >= 9 => {
                                        let eq_slot_raw = u32::from_be_bytes([
                                            payload[1], payload[2], payload[3], payload[4],
                                        ])
                                            as u8;

                                        if let Some(slot) = EquipmentSlot::from_u8(eq_slot_raw) {
                                            if let Some(player) = self
                                                .entities
                                                .values_mut()
                                                .find(|e| e.peer_addr == Some(peer))
                                            {
                                                let player_id = player.id;
                                                let account_id = player.account_id;

                                                if let Some(aid) = account_id {
                                                    if let Some(account) =
                                                        self.tx_manager.get_account_mut(aid)
                                                    {
                                                        let _ = account.unequip_item(slot);
                                                    }
                                                }

                                                let _ = player.equipment.unequip(slot);
                                                self.broadcast_equipment_changed(
                                                    player_id,
                                                    eq_slot_raw,
                                                    0,
                                                );
                                            }
                                        }
                                    }

                                    // 5: Send chat [5, channel (1B), target_id (4B), text_len (2B), text...]
                                    5 if payload.len() >= 8 => {
                                        let channel_raw = payload[1];
                                        let target_id = u32::from_be_bytes([
                                            payload[2], payload[3], payload[4], payload[5],
                                        ]);
                                        let text_len =
                                            u16::from_be_bytes([payload[6], payload[7]]) as usize;

                                        if payload.len() >= 8 + text_len {
                                            let text_bytes = &payload[8..8 + text_len];

                                            if let Some(sender) = self
                                                .entities
                                                .values()
                                                .find(|e| e.peer_addr == Some(peer))
                                            {
                                                let sender_id = sender.id;
                                                let sender_pos = sender.position;
                                                let sender_account =
                                                    sender.account_id.unwrap_or(sender_id as u64);

                                                if self
                                                    .chat_limiter
                                                    .check_and_consume(
                                                        sender_account,
                                                        self.current_tick,
                                                    )
                                                    .is_ok()
                                                {
                                                    self.route_chat_message(
                                                        channel_raw,
                                                        sender_id,
                                                        sender_pos,
                                                        sender_account,
                                                        target_id,
                                                        text_bytes,
                                                    );
                                                }
                                            }
                                        }
                                    }

                                    // 6: Party command [6, cmd (1B), target_account (8B)]
                                    6 if payload.len() >= 10 => {
                                        let cmd = payload[1];
                                        let target_account = u64::from_be_bytes([
                                            payload[2], payload[3], payload[4], payload[5],
                                            payload[6], payload[7], payload[8], payload[9],
                                        ]);

                                        if let Some(player) = self
                                            .entities
                                            .values()
                                            .find(|e| e.peer_addr == Some(peer))
                                        {
                                            let caller_account =
                                                player.account_id.unwrap_or(player.id as u64);
                                            let player_id = player.id;
                                            let health = player.health;
                                            let max_health = player.max_health;
                                            let mana = player.mana;
                                            let max_mana = player.max_mana;
                                            let position = player.position;

                                            match cmd {
                                                // 1 = Invite
                                                1 => {
                                                    let caller_member = PartyMember {
                                                        account_id: caller_account,
                                                        entity_id: player_id,
                                                        name: format!("Player{player_id}"),
                                                        health,
                                                        max_health,
                                                        mana,
                                                        max_mana,
                                                        position,
                                                    };
                                                    let party_id = match self
                                                        .party_manager
                                                        .get_party_by_account(caller_account)
                                                    {
                                                        Some(p) => p.party_id,
                                                        None => self
                                                            .party_manager
                                                            .create_party(caller_member)
                                                            .unwrap_or(0),
                                                    };

                                                    if party_id > 0 {
                                                        let _ = self.party_manager.invite_player(
                                                            party_id,
                                                            caller_account,
                                                            target_account,
                                                        );
                                                        self.send_party_updated_to_peer(
                                                            peer,
                                                            party_id,
                                                            caller_account,
                                                            1,
                                                        );
                                                    }
                                                }
                                                // 2 = Accept
                                                2 => {
                                                    let member = PartyMember {
                                                        account_id: caller_account,
                                                        entity_id: player_id,
                                                        name: format!("Player{player_id}"),
                                                        health,
                                                        max_health,
                                                        mana,
                                                        max_mana,
                                                        position,
                                                    };
                                                    if let Ok(party_id) = self
                                                        .party_manager
                                                        .accept_invite(caller_account, member)
                                                    {
                                                        if let Some(party) =
                                                            self.party_manager.get_party(party_id)
                                                        {
                                                            let leader = party.leader_account_id;
                                                            let count = party.members.len() as u8;
                                                            self.broadcast_party_updated(
                                                                party_id, leader, count,
                                                            );
                                                        }
                                                    }
                                                }
                                                // 3 = Leave
                                                3 => {
                                                    if let Ok(Some(party_id)) = self
                                                        .party_manager
                                                        .leave_party(caller_account)
                                                    {
                                                        if let Some(party) =
                                                            self.party_manager.get_party(party_id)
                                                        {
                                                            let leader = party.leader_account_id;
                                                            let count = party.members.len() as u8;
                                                            self.broadcast_party_updated(
                                                                party_id, leader, count,
                                                            );
                                                        }
                                                        self.send_party_updated_to_peer(
                                                            peer, 0, 0, 0,
                                                        );
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    _ => {}
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

    fn route_chat_message(
        &self,
        channel_raw: u8,
        sender_id: u32,
        sender_pos: Vec3Fix,
        sender_account: u64,
        target_id: u32,
        text_bytes: &[u8],
    ) {
        let channel = match ChatChannel::from_u8(channel_raw) {
            Some(c) => c,
            None => return,
        };

        // Format ChatReceived packet: [6, channel (1B), sender_id (4B), text_len (2B), text...]
        let text_len = text_bytes.len().min(u16::MAX as usize) as u16;
        let mut msg_packet = Vec::with_capacity(8 + text_bytes.len());
        msg_packet.push(6);
        msg_packet.push(channel_raw);
        msg_packet.extend_from_slice(&sender_id.to_be_bytes());
        msg_packet.extend_from_slice(&text_len.to_be_bytes());
        msg_packet.extend_from_slice(&text_bytes[..text_len as usize]);

        match channel {
            ChatChannel::SpatialProximity => {
                // Broadcast to players within 25 meters (25^2 = 625)
                let mut query_buf = [0u32; 128];
                let res = self.spatial_grid.query_radius_squared(
                    sender_pos,
                    Fixed64::from_i32(625),
                    &mut query_buf,
                );

                for &eid in &query_buf[..res.written] {
                    if let Some(peer) = self.entities.get(&eid).and_then(|e| e.peer_addr) {
                        self.send_reliable_event_to_peer(peer, &msg_packet);
                    }
                }
            }
            ChatChannel::Party => {
                if let Some(party) = self.party_manager.get_party_by_account(sender_account) {
                    for member in &party.members {
                        if let Some(peer) = self
                            .entities
                            .values()
                            .find(|e| e.account_id == Some(member.account_id))
                            .and_then(|e| e.peer_addr)
                        {
                            self.send_reliable_event_to_peer(peer, &msg_packet);
                        }
                    }
                }
            }
            ChatChannel::Whisper => {
                if let Some(target_peer) = self.entities.get(&target_id).and_then(|e| e.peer_addr) {
                    self.send_reliable_event_to_peer(target_peer, &msg_packet);
                }
                if let Some(sender_peer) = self.entities.get(&sender_id).and_then(|e| e.peer_addr) {
                    self.send_reliable_event_to_peer(sender_peer, &msg_packet);
                }
            }
            ChatChannel::GlobalShout => {
                for entity in self.entities.values() {
                    if let Some(peer) = entity.peer_addr {
                        self.send_reliable_event_to_peer(peer, &msg_packet);
                    }
                }
            }
        }
    }

    fn broadcast_aoi_updates(&mut self) {
        // Replenish bandwidth governor tokens for this tick
        self.bandwidth_governor.tick();

        let observers: Vec<(u32, SocketAddr, Vec3Fix)> = self
            .entities
            .values()
            .filter_map(|e| e.peer_addr.map(|addr| (e.id, addr, e.position)))
            .collect();

        let mut query_buf = [0u32; 2048];
        for (player_id, peer_addr, pos) in observers {
            let max_r_sq = if self.density_profile == DensityProfile::BudgetMobile {
                Fixed64::from_i32(2500)
            } else {
                Fixed64::from_i32(90000)
            };

            let query_res = self
                .spatial_grid
                .query_radius_morton(pos, max_r_sq, &mut query_buf);

            let mut out_packet = [0u8; MAX_PACKET_SIZE];
            let mut header = PacketHeader::new(
                ChannelType::UnreliableSequenced,
                PacketType::StateUpdate,
                self.current_tick as u16,
                0,
                0,
            );

            if let Ok(hdr_len) = header.write_to(&mut out_packet[..HEADER_SIZE]) {
                let mut p_idx = hdr_len;

                for &visible_id in &query_buf[..query_res.written] {
                    // Check interior cell pocket-dimension isolation:
                    // Entities inside an interior room are completely invisible to outside players,
                    // and players inside a room only see occupants of the exact same room.
                    let player_cell = self.interior_manager.get_entity_cell(player_id);
                    let target_cell = self.interior_manager.get_entity_cell(visible_id);
                    if player_cell != target_cell {
                        continue;
                    }

                    if let Some(target) = self.entities.get(&visible_id) {
                        let dist_sq = pos.distance_squared(target.position);
                        let tier = FrequencyTier::classify_5tier(dist_sq);
                        let tier_idx = tier as u8;
                        const RECORD_LEN: usize = 22;

                        if !self
                            .bandwidth_governor
                            .should_admit_tier(player_id, tier_idx, RECORD_LEN)
                        {
                            continue;
                        }

                        if p_idx + RECORD_LEN > MAX_PACKET_SIZE {
                            if p_idx > hdr_len {
                                let sent_len = p_idx;
                                if self.bandwidth_governor.consume(player_id, sent_len) {
                                    let _ = self.socket.send_to(&out_packet[..sent_len], peer_addr);
                                }
                            }
                            if !self
                                .bandwidth_governor
                                .can_send(player_id, hdr_len + RECORD_LEN)
                            {
                                break;
                            }
                            header.sequence = header.sequence.wrapping_add(1);
                            let _ = header.write_to(&mut out_packet[..HEADER_SIZE]);
                            p_idx = hdr_len;
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
                    let sent_len = p_idx;
                    if self.bandwidth_governor.consume(player_id, sent_len) {
                        let _ = self.socket.send_to(&out_packet[..sent_len], peer_addr);
                    }
                }
            }
        }
    }

    fn send_reliable_event_to_peer(&self, peer: SocketAddr, payload: &[u8]) {
        let header = PacketHeader::new(
            ChannelType::ReliableOrdered,
            PacketType::ReliableMessage,
            self.current_tick as u16,
            0,
            0,
        );
        let mut wire = [0u8; MAX_PACKET_SIZE];
        if let Ok(hlen) = header.write_to(&mut wire) {
            if hlen + payload.len() <= MAX_PACKET_SIZE {
                wire[hlen..hlen + payload.len()].copy_from_slice(payload);
                let _ = self.socket.send_to(&wire[..hlen + payload.len()], peer);
            }
        }
    }

    fn broadcast_cast_started(&self, caster_id: u32, ability_id: u32, duration_ticks: u32) {
        let mut payload = [0u8; 13];
        payload[0] = 3; // CastStarted
        payload[1..5].copy_from_slice(&caster_id.to_be_bytes());
        payload[5..9].copy_from_slice(&ability_id.to_be_bytes());
        payload[9..13].copy_from_slice(&duration_ticks.to_be_bytes());

        for entity in self.entities.values() {
            if let Some(peer) = entity.peer_addr {
                self.send_reliable_event_to_peer(peer, &payload);
            }
        }
    }

    fn broadcast_cast_interrupted(&self, caster_id: u32, ability_id: u32, reason: u8) {
        let mut payload = [0u8; 10];
        payload[0] = 4; // CastInterrupted
        payload[1..5].copy_from_slice(&caster_id.to_be_bytes());
        payload[5..9].copy_from_slice(&ability_id.to_be_bytes());
        payload[9] = reason;

        for entity in self.entities.values() {
            if let Some(peer) = entity.peer_addr {
                self.send_reliable_event_to_peer(peer, &payload);
            }
        }
    }

    fn broadcast_cast_completed(&self, caster_id: u32, ability_id: u32) {
        let mut payload = [0u8; 9];
        payload[0] = 5; // CastCompleted
        payload[1..5].copy_from_slice(&caster_id.to_be_bytes());
        payload[5..9].copy_from_slice(&ability_id.to_be_bytes());

        for entity in self.entities.values() {
            if let Some(peer) = entity.peer_addr {
                self.send_reliable_event_to_peer(peer, &payload);
            }
        }
    }

    fn broadcast_combat_action(&self, source_id: u32, target_id: u32, action: u8, value: u32) {
        let mut payload = [0u8; 14];
        payload[0] = 1; // CombatAction
        payload[1..5].copy_from_slice(&source_id.to_be_bytes());
        payload[5..9].copy_from_slice(&target_id.to_be_bytes());
        payload[9] = action;
        payload[10..14].copy_from_slice(&value.to_be_bytes());

        for entity in self.entities.values() {
            if let Some(peer) = entity.peer_addr {
                self.send_reliable_event_to_peer(peer, &payload);
            }
        }
    }

    fn broadcast_equipment_changed(&self, entity_id: u32, slot: u8, item_id: u32) {
        let mut payload = [0u8; 10];
        payload[0] = 7; // EquipmentChanged
        payload[1..5].copy_from_slice(&entity_id.to_be_bytes());
        payload[5] = slot;
        payload[6..10].copy_from_slice(&item_id.to_be_bytes());

        for entity in self.entities.values() {
            if let Some(peer) = entity.peer_addr {
                self.send_reliable_event_to_peer(peer, &payload);
            }
        }
    }

    fn send_party_updated_to_peer(
        &self,
        peer: SocketAddr,
        party_id: u64,
        leader_id: u64,
        count: u8,
    ) {
        let mut payload = [0u8; 18];
        payload[0] = 8; // PartyUpdated
        payload[1..9].copy_from_slice(&party_id.to_be_bytes());
        payload[9..17].copy_from_slice(&leader_id.to_be_bytes());
        payload[17] = count;
        self.send_reliable_event_to_peer(peer, &payload);
    }

    fn broadcast_party_updated(&self, party_id: u64, leader_id: u64, count: u8) {
        if let Some(party) = self.party_manager.get_party(party_id) {
            for member in &party.members {
                if let Some(peer) = self
                    .entities
                    .values()
                    .find(|e| e.account_id == Some(member.account_id))
                    .and_then(|e| e.peer_addr)
                {
                    self.send_party_updated_to_peer(peer, party_id, leader_id, count);
                }
            }
        }
    }

    fn send_loot_event(&self, peer: SocketAddr, entity_id: u32, item_id: u32, amount: u32) {
        let mut payload = [0u8; 13];
        payload[0] = 2; // LootAcquired
        payload[1..5].copy_from_slice(&entity_id.to_be_bytes());
        payload[5..9].copy_from_slice(&item_id.to_be_bytes());
        payload[9..13].copy_from_slice(&amount.to_be_bytes());
        self.send_reliable_event_to_peer(peer, &payload);
    }

    /// Enables continental topology and dynamic multi-shard orchestration.
    pub fn enable_continental_topology(
        &mut self,
        local_shard_id: u32,
        num_shards: usize,
    ) -> Result<(), AppError> {
        let orchestrator = ContinentalOrchestrator::new(local_shard_id, num_shards)?;
        self.continental_orchestrator = Some(orchestrator);
        Ok(())
    }

    /// Returns a reference to the continental orchestrator if enabled.
    pub fn continental_orchestrator(&self) -> Option<&ContinentalOrchestrator> {
        self.continental_orchestrator.as_ref()
    }

    /// Returns a mutable reference to the continental orchestrator if enabled.
    pub fn continental_orchestrator_mut(&mut self) -> Option<&mut ContinentalOrchestrator> {
        self.continental_orchestrator.as_mut()
    }

    /// Assigns a 256m sector to a worker shard within the continental topology.
    pub fn assign_continental_sector(
        &mut self,
        sector_x: i32,
        sector_z: i32,
        shard_id: u32,
    ) -> Result<(), AppError> {
        let orch = self
            .continental_orchestrator
            .as_mut()
            .ok_or(AppError::World(WorldError::ShardNotFound(shard_id)))?;
        orch.shard_manager_mut()
            .assign_sector(sector_x, sector_z, shard_id)
            .map_err(AppError::World)
    }

    /// Queries the worker shard currently assigned to a 256m continental sector.
    pub fn query_shard_for_sector(&self, sector_x: i32, sector_z: i32) -> Option<u32> {
        self.continental_orchestrator
            .as_ref()?
            .shard_manager()
            .get_shard_for_sector(sector_x, sector_z)
    }

    /// Loads a 512m macro terrain tile into the continental HLOD grid.
    pub fn load_macro_terrain_tile(&mut self, tile: TerrainTile) -> Result<(), AppError> {
        let orch = self
            .continental_orchestrator
            .as_mut()
            .ok_or(AppError::World(WorldError::TerrainTileNotFound(0, 0)))?;
        orch.load_terrain_tile(tile);
        Ok(())
    }

    /// Samples macro terrain ground elevation at a hierarchical global coordinate.
    pub fn sample_terrain_elevation(&mut self, coord: &GlobalCoord) -> Option<Fixed64> {
        let orch = self.continental_orchestrator.as_mut()?;
        orch.sample_terrain_elevation(coord)
    }

    /// Raycasts against continental terrain heightfields.
    pub fn raycast_terrain(
        &mut self,
        origin: &GlobalCoord,
        dir: Vec3Fix,
        max_distance: Fixed64,
    ) -> Option<RayHit> {
        let orch = self.continental_orchestrator.as_mut()?;
        orch.raycast_terrain(origin, dir, max_distance)
    }

    /// Evaluates entity movement and issues a predictive boundary migration pre-auth token.
    pub fn evaluate_predictive_migration(
        &mut self,
        entity_id: u32,
        coord: GlobalCoord,
        velocity: Vec3Fix,
    ) -> Option<PredictiveMigrationPreAuth> {
        let orch = self.continental_orchestrator.as_mut()?;
        orch.evaluate_entity_movement(entity_id, coord, velocity)
    }

    /// Evaluates high-speed vehicle movement and issues a predictive boundary migration pre-auth token.
    pub fn evaluate_vehicle_predictive_migration(
        &mut self,
        entity_id: u32,
        coord: GlobalCoord,
        vehicle: &VehicleKinematics,
    ) -> Option<PredictiveMigrationPreAuth> {
        let orch = self.continental_orchestrator.as_mut()?;
        orch.evaluate_vehicle_movement(entity_id, coord, vehicle)
    }
}
