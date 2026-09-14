//! Struct-of-Arrays (SoA) world storage and cache-aligned memory layout.
//!
//! Reorganizes active entities into contiguous parallel vectors to maximize L1 data cache
//! utilization during dense 20 Hz tick simulation and physics extrapolation loops.

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::quant::QuantizedYaw;
use eidolon_core::simd::{Vec3Fix16x, Vec3Fix8x};

use crate::equipment::EquipmentContainer;
use crate::error::WorldError;

/// Sentinel index representing an inactive or unallocated entity in the sparse mapping table.
pub const SPARSE_SENTINEL: u32 = u32::MAX;

/// Cold entity metadata decoupled from the hot fixed-frequency simulation tick loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColdEntityMetadata {
    /// Associated persistent player account ID, if applicable.
    pub account_id: Option<u64>,
    /// Active network session ID.
    pub session_id: Option<u64>,
    /// Associated guild identifier.
    pub guild_id: Option<u32>,
    /// Equipped character gear.
    pub equipment: EquipmentContainer,
    /// Display name of the character.
    pub name: String,
}

impl Default for ColdEntityMetadata {
    fn default() -> Self {
        Self {
            account_id: None,
            session_id: None,
            guild_id: None,
            equipment: EquipmentContainer::new(),
            name: String::new(),
        }
    }
}

/// Parameters for spawning an entity into Struct-of-Arrays storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntitySpawnParams {
    /// Authoritative entity ID.
    pub id: u32,
    /// Initial world position.
    pub position: Vec3Fix,
    /// Initial velocity.
    pub velocity: Vec3Fix,
    /// Initial acceleration.
    pub acceleration: Vec3Fix,
    /// Facing heading.
    pub heading: QuantizedYaw,
    /// Movement flags.
    pub flags: u8,
    /// Initial health.
    pub health: u32,
    /// Maximum health.
    pub max_health: u32,
    /// Cold metadata.
    pub cold: ColdEntityMetadata,
}

impl EntitySpawnParams {
    /// Creates a new spawn parameter bundle with default health, metadata, and zero acceleration.
    pub fn new(id: u32, position: Vec3Fix, velocity: Vec3Fix, heading: QuantizedYaw) -> Self {
        Self {
            id,
            position,
            velocity,
            acceleration: Vec3Fix::ZERO,
            heading,
            flags: 0,
            health: 100,
            max_health: 100,
            cold: ColdEntityMetadata::default(),
        }
    }

    /// Creates a new spawn parameter bundle with explicit acceleration.
    pub fn with_acceleration(
        id: u32,
        position: Vec3Fix,
        velocity: Vec3Fix,
        acceleration: Vec3Fix,
        heading: QuantizedYaw,
    ) -> Self {
        Self {
            id,
            position,
            velocity,
            acceleration,
            heading,
            flags: 0,
            health: 100,
            max_health: 100,
            cold: ColdEntityMetadata::default(),
        }
    }
}

/// Slice view over core entity kinematic components.
pub type EntityComponentSlices<'a> = (
    &'a [u32],
    &'a [Vec3Fix],
    &'a [Vec3Fix],
    &'a [QuantizedYaw],
    &'a [u8],
);

/// Slice view over 2nd-order entity kinematic components (id, pos, vel, accel, yaw, flags).
pub type EntityComponent2ndOrderSlices<'a> = (
    &'a [u32],
    &'a [Vec3Fix],
    &'a [Vec3Fix],
    &'a [Vec3Fix],
    &'a [QuantizedYaw],
    &'a [u8],
);

/// Struct-of-Arrays storage for active entities in the simulation world.
///
/// Segregates hot transform and kinematic data into contiguous parallel vectors
/// and manages dense-to-sparse mapping with O(1) swap-remove despawning.
#[derive(Debug, Clone)]
pub struct SoaEntityStorage {
    // --- Hot Contiguous Component Arrays ---
    /// Contiguous entity identifiers.
    ids: Vec<u32>,
    /// Contiguous 32.32 fixed-point world positions.
    positions: Vec<Vec3Fix>,
    /// Contiguous velocity vectors in meters per second.
    velocities: Vec<Vec3Fix>,
    /// Contiguous acceleration vectors in meters per second squared.
    accelerations: Vec<Vec3Fix>,
    /// Contiguous facing headings.
    headings: Vec<QuantizedYaw>,
    /// Movement state and status flags.
    flags: Vec<u8>,
    /// Current health points.
    health: Vec<u32>,
    /// Maximum health points.
    max_health: Vec<u32>,

    // --- Cold Component Arrays ---
    /// Decoupled metadata never accessed in hot kinematic tick loops.
    cold_data: Vec<ColdEntityMetadata>,

    // --- Sparse-to-Dense Index Mapping ---
    /// Direct index table mapping `entity_id` to its index in the dense arrays.
    sparse_to_dense: Vec<u32>,
}

impl SoaEntityStorage {
    /// Constructs a new Struct-of-Arrays storage with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            ids: Vec::with_capacity(capacity),
            positions: Vec::with_capacity(capacity),
            velocities: Vec::with_capacity(capacity),
            accelerations: Vec::with_capacity(capacity),
            headings: Vec::with_capacity(capacity),
            flags: Vec::with_capacity(capacity),
            health: Vec::with_capacity(capacity),
            max_health: Vec::with_capacity(capacity),
            cold_data: Vec::with_capacity(capacity),
            sparse_to_dense: Vec::new(),
        }
    }

    /// Returns the number of active entities currently stored in the dense arrays.
    #[inline]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Returns true if no entities are currently stored.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Returns the allocated capacity of the dense parallel arrays.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.ids.capacity()
    }

    /// Spawns a new entity into the packed contiguous storage.
    ///
    /// Returns the dense array index assigned to the entity, or an error if the ID already exists.
    pub fn spawn(&mut self, params: EntitySpawnParams) -> Result<usize, WorldError> {
        let id_idx = params.id as usize;
        if id_idx < self.sparse_to_dense.len() && self.sparse_to_dense[id_idx] != SPARSE_SENTINEL {
            return Err(WorldError::EntityAlreadyExists(params.id));
        }

        // Expand sparse mapping table if ID exceeds current bounds
        if id_idx >= self.sparse_to_dense.len() {
            self.sparse_to_dense.resize(id_idx + 1, SPARSE_SENTINEL);
        }

        let dense_idx = self.ids.len();
        self.ids.push(params.id);
        self.positions.push(params.position);
        self.velocities.push(params.velocity);
        self.accelerations.push(params.acceleration);
        self.headings.push(params.heading);
        self.flags.push(params.flags);
        self.health.push(params.health);
        self.max_health.push(params.max_health);
        self.cold_data.push(params.cold);

        self.sparse_to_dense[id_idx] = dense_idx as u32;

        Ok(dense_idx)
    }

    /// Despawns an entity in O(1) time using swap-remove to maintain packed contiguous memory.
    ///
    /// Returns the decoupled cold metadata of the removed entity.
    pub fn despawn(&mut self, id: u32) -> Result<ColdEntityMetadata, WorldError> {
        let id_idx = id as usize;
        let dense_idx = if id_idx < self.sparse_to_dense.len() {
            let idx = self.sparse_to_dense[id_idx];
            if idx == SPARSE_SENTINEL {
                return Err(WorldError::EntityNotFound(id));
            }
            idx as usize
        } else {
            return Err(WorldError::EntityNotFound(id));
        };

        let last_idx = self.ids.len().saturating_sub(1);

        // Perform swap-remove on all dense parallel vectors
        let removed_id = self.ids.swap_remove(dense_idx);
        let _pos = self.positions.swap_remove(dense_idx);
        let _vel = self.velocities.swap_remove(dense_idx);
        let _acc = self.accelerations.swap_remove(dense_idx);
        let _yaw = self.headings.swap_remove(dense_idx);
        let _flg = self.flags.swap_remove(dense_idx);
        let _hp = self.health.swap_remove(dense_idx);
        let _mhp = self.max_health.swap_remove(dense_idx);
        let removed_cold = self.cold_data.swap_remove(dense_idx);

        // Clear sparse entry for despawned entity
        self.sparse_to_dense[removed_id as usize] = SPARSE_SENTINEL;

        // If an entity was swapped into the vacancy, update its sparse map pointer
        if dense_idx < last_idx {
            let swapped_id = self.ids[dense_idx] as usize;
            self.sparse_to_dense[swapped_id] = dense_idx as u32;
        }

        Ok(removed_cold)
    }

    /// Performs contiguous linear memory physics extrapolation across all active entities.
    ///
    /// Updates positions: `pos = pos + vel * dt` with 100% cache line utilization and zero branching,
    /// accelerated by 16-lane SIMD registers.
    pub fn step_kinematics(&mut self, dt: Fixed64) {
        self.step_kinematics_simd_16x(dt);
    }

    /// Performs vectorized 16-wide physics extrapolation across active entities using 512-bit SIMD registers.
    ///
    /// Processes chunks of 16 entities using `Vec3Fix16x::step_kinematics_chunk` with AVX-512 / dual NEON parallelism,
    /// falls back to an 8-wide chunk if remaining >= 8, and processes remaining tail entities (< 8)
    /// sequentially with exact mathematical parity.
    pub fn step_kinematics_simd_16x(&mut self, dt: Fixed64) {
        let count = self.positions.len();
        let chunks_16 = count / 16;
        for c in 0..chunks_16 {
            let start = c * 16;
            let end = start + 16;
            if let (Ok(pos_chunk), Ok(vel_chunk)) = (
                <&mut [Vec3Fix; 16]>::try_from(&mut self.positions[start..end]),
                <&[Vec3Fix; 16]>::try_from(&self.velocities[start..end]),
            ) {
                Vec3Fix16x::step_kinematics_chunk(pos_chunk, vel_chunk, dt);
            }
        }

        let mut offset = chunks_16 * 16;
        if count - offset >= 8 {
            let start = offset;
            let end = start + 8;
            if let (Ok(pos_chunk), Ok(vel_chunk)) = (
                <&mut [Vec3Fix; 8]>::try_from(&mut self.positions[start..end]),
                <&[Vec3Fix; 8]>::try_from(&self.velocities[start..end]),
            ) {
                Vec3Fix8x::step_kinematics_chunk(pos_chunk, vel_chunk, dt);
            }
            offset += 8;
        }

        // Tail elements (< 8)
        for i in offset..count {
            let displacement = self.velocities[i] * dt;
            self.positions[i] += displacement;
        }
    }

    /// Performs vectorized 8-wide physics extrapolation across active entities using SIMD registers.
    ///
    /// Processes chunks of 8 entities using `Vec3Fix8x::step_kinematics_chunk` with AVX2/NEON parallelism,
    /// and processes remaining tail entities (< 8) sequentially with exact mathematical parity.
    pub fn step_kinematics_simd_8x(&mut self, dt: Fixed64) {
        let count = self.positions.len();
        let chunks = count / 8;
        for c in 0..chunks {
            let start = c * 8;
            let end = start + 8;
            if let (Ok(pos_chunk), Ok(vel_chunk)) = (
                <&mut [Vec3Fix; 8]>::try_from(&mut self.positions[start..end]),
                <&[Vec3Fix; 8]>::try_from(&self.velocities[start..end]),
            ) {
                Vec3Fix8x::step_kinematics_chunk(pos_chunk, vel_chunk, dt);
            }
        }

        // Tail elements (< 8)
        for i in (chunks * 8)..count {
            let displacement = self.velocities[i] * dt;
            self.positions[i] += displacement;
        }
    }

    /// Performs scalar physics extrapolation across all active entities for baseline and parity testing.
    pub fn step_kinematics_scalar(&mut self, dt: Fixed64) {
        let count = self.positions.len();
        for i in 0..count {
            let displacement = self.velocities[i] * dt;
            self.positions[i] += displacement;
        }
    }

    /// Performs 2nd-order quadratic physics extrapolation:
    /// pos = pos + vel * dt + 0.5 * accel * dt^2
    /// vel = vel + accel * dt
    pub fn step_kinematics_2nd_order(&mut self, dt: Fixed64) {
        let half_dt_sq = dt * dt * Fixed64::HALF;
        let count = self.positions.len();
        for i in 0..count {
            let displacement = (self.velocities[i] * dt) + (self.accelerations[i] * half_dt_sq);
            self.positions[i] += displacement;
            self.velocities[i] += self.accelerations[i] * dt;
        }
    }

    /// Retrieves an entity's transform components by entity ID in O(1) time.
    #[inline]
    pub fn get_transform(&self, id: u32) -> Option<(Vec3Fix, Vec3Fix, QuantizedYaw)> {
        let id_idx = id as usize;
        if id_idx < self.sparse_to_dense.len() {
            let dense_idx = self.sparse_to_dense[id_idx];
            if dense_idx != SPARSE_SENTINEL {
                let idx = dense_idx as usize;
                return Some((
                    self.positions[idx],
                    self.velocities[idx],
                    self.headings[idx],
                ));
            }
        }
        None
    }

    /// Retrieves an entity's 2nd-order transform components (pos, vel, accel, yaw) by entity ID.
    #[inline]
    pub fn get_transform_2nd_order(
        &self,
        id: u32,
    ) -> Option<(Vec3Fix, Vec3Fix, Vec3Fix, QuantizedYaw)> {
        let id_idx = id as usize;
        if id_idx < self.sparse_to_dense.len() {
            let dense_idx = self.sparse_to_dense[id_idx];
            if dense_idx != SPARSE_SENTINEL {
                let idx = dense_idx as usize;
                return Some((
                    self.positions[idx],
                    self.velocities[idx],
                    self.accelerations[idx],
                    self.headings[idx],
                ));
            }
        }
        None
    }

    /// Updates an entity's position and heading in O(1) time.
    pub fn set_transform(
        &mut self,
        id: u32,
        pos: Vec3Fix,
        vel: Vec3Fix,
        heading: QuantizedYaw,
    ) -> bool {
        let id_idx = id as usize;
        if id_idx < self.sparse_to_dense.len() {
            let dense_idx = self.sparse_to_dense[id_idx];
            if dense_idx != SPARSE_SENTINEL {
                let idx = dense_idx as usize;
                self.positions[idx] = pos;
                self.velocities[idx] = vel;
                self.headings[idx] = heading;
                return true;
            }
        }
        false
    }

    /// Updates an entity's 2nd-order transform (pos, vel, accel, heading) in O(1) time.
    pub fn set_transform_2nd_order(
        &mut self,
        id: u32,
        pos: Vec3Fix,
        vel: Vec3Fix,
        accel: Vec3Fix,
        heading: QuantizedYaw,
    ) -> bool {
        let id_idx = id as usize;
        if id_idx < self.sparse_to_dense.len() {
            let dense_idx = self.sparse_to_dense[id_idx];
            if dense_idx != SPARSE_SENTINEL {
                let idx = dense_idx as usize;
                self.positions[idx] = pos;
                self.velocities[idx] = vel;
                self.accelerations[idx] = accel;
                self.headings[idx] = heading;
                return true;
            }
        }
        false
    }

    /// Returns the dense array index for an entity ID if present in the storage.
    #[inline]
    pub fn lookup_index(&self, id: u32) -> Option<usize> {
        let id_idx = id as usize;
        if id_idx < self.sparse_to_dense.len() {
            let dense_idx = self.sparse_to_dense[id_idx];
            if dense_idx != SPARSE_SENTINEL {
                return Some(dense_idx as usize);
            }
        }
        None
    }

    /// Provides immutable slice view over the entity positions.
    #[inline]
    pub fn positions(&self) -> &[Vec3Fix] {
        &self.positions
    }

    /// Provides mutable slice view over the entity positions.
    #[inline]
    pub fn positions_mut(&mut self) -> &mut [Vec3Fix] {
        &mut self.positions
    }

    /// Provides immutable slice view over the entity velocities.
    #[inline]
    pub fn velocities(&self) -> &[Vec3Fix] {
        &self.velocities
    }

    /// Provides mutable slice view over the entity velocities.
    #[inline]
    pub fn velocities_mut(&mut self) -> &mut [Vec3Fix] {
        &mut self.velocities
    }

    /// Provides immutable slice view over the entity accelerations.
    #[inline]
    pub fn accelerations(&self) -> &[Vec3Fix] {
        &self.accelerations
    }

    /// Provides mutable slice view over the entity accelerations.
    #[inline]
    pub fn accelerations_mut(&mut self) -> &mut [Vec3Fix] {
        &mut self.accelerations
    }

    /// Provides immutable slice views over the dense parallel arrays for bulk processing.
    #[inline]
    pub fn components(&self) -> EntityComponentSlices<'_> {
        (
            &self.ids,
            &self.positions,
            &self.velocities,
            &self.headings,
            &self.flags,
        )
    }

    /// Provides immutable slice views over the 2nd-order kinematic components.
    #[inline]
    pub fn components_2nd_order(&self) -> EntityComponent2ndOrderSlices<'_> {
        (
            &self.ids,
            &self.positions,
            &self.velocities,
            &self.accelerations,
            &self.headings,
            &self.flags,
        )
    }

    /// Provides mutable slice views over health and max health for combat calculations.
    #[inline]
    pub fn health_mut(&mut self) -> (&mut [u32], &[u32]) {
        (&mut self.health, &self.max_health)
    }

    /// Returns current and maximum health points of an entity if present.
    pub fn get_health(&self, id: u32) -> Option<(u32, u32)> {
        let id_idx = id as usize;
        if id_idx < self.sparse_to_dense.len() {
            let dense_idx = self.sparse_to_dense[id_idx];
            if dense_idx != SPARSE_SENTINEL {
                let idx = dense_idx as usize;
                return Some((self.health[idx], self.max_health[idx]));
            }
        }
        None
    }

    /// Returns a reference to an entity's cold metadata if present.
    pub fn get_cold_data(&self, id: u32) -> Option<&ColdEntityMetadata> {
        let id_idx = id as usize;
        if id_idx < self.sparse_to_dense.len() {
            let dense_idx = self.sparse_to_dense[id_idx];
            if dense_idx != SPARSE_SENTINEL {
                return self.cold_data.get(dense_idx as usize);
            }
        }
        None
    }

    /// Returns a mutable reference to an entity's cold metadata if present.
    pub fn get_cold_data_mut(&mut self, id: u32) -> Option<&mut ColdEntityMetadata> {
        let id_idx = id as usize;
        if id_idx < self.sparse_to_dense.len() {
            let dense_idx = self.sparse_to_dense[id_idx];
            if dense_idx != SPARSE_SENTINEL {
                return self.cold_data.get_mut(dense_idx as usize);
            }
        }
        None
    }
    /// Exports all active entities into contiguous 64-byte cache-line aligned blocks.
    pub fn to_aligned_blocks(&self) -> Vec<AlignedEntityBlock64> {
        let count = self.ids.len();
        let mut blocks = Vec::with_capacity(count);
        for i in 0..count {
            blocks.push(AlignedEntityBlock64 {
                position: self.positions[i],
                velocity: self.velocities[i],
                entity_id: self.ids[i],
                heading: self.headings[i],
                flags: self.flags[i],
                health: self.health[i].min(u16::MAX as u32) as u16,
                max_health: self.max_health[i].min(u16::MAX as u32) as u16,
                generation: 0,
                _reserved: [0; 4],
            });
        }
        blocks
    }

    /// Populates positions, velocities, headings, and flags from updated 64-byte aligned blocks.
    pub fn update_from_aligned_blocks(&mut self, blocks: &[AlignedEntityBlock64]) {
        let count = self.ids.len().min(blocks.len());
        for (i, block) in blocks.iter().enumerate().take(count) {
            self.positions[i] = block.position;
            self.velocities[i] = block.velocity;
            self.headings[i] = block.heading;
            self.flags[i] = block.flags;
        }
    }
}

/// Individual entity kinematic block aligned to a 64-byte hardware CPU cache line.
///
/// Ensures that loading an entity's position pre-fetches velocity, heading, flags, and health
/// into L1 data cache in a single 64-byte memory transaction with zero false sharing or split-cache stalls.
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlignedEntityBlock64 {
    /// 3D position in 32.32 fixed-point (24 bytes).
    pub position: Vec3Fix,
    /// 3D velocity in 32.32 fixed-point (24 bytes).
    pub velocity: Vec3Fix,
    /// Authoritative entity identifier (4 bytes).
    pub entity_id: u32,
    /// Facing yaw heading (1 byte).
    pub heading: QuantizedYaw,
    /// Movement state and flags (1 byte).
    pub flags: u8,
    /// Current health points (2 bytes).
    pub health: u16,
    /// Maximum health points (2 bytes).
    pub max_health: u16,
    /// Lifecycle or sync generation counter (2 bytes).
    pub generation: u16,
    /// Reserved padding ensuring exact 64-byte size (4 bytes).
    pub _reserved: [u8; 4],
}

// Compile-time assertions verifying exact 64-byte size and alignment.
const _: () = assert!(core::mem::size_of::<AlignedEntityBlock64>() == 64);
const _: () = assert!(core::mem::align_of::<AlignedEntityBlock64>() == 64);

impl AlignedEntityBlock64 {
    /// Constructs a new 64-byte aligned entity block with default health and generation.
    pub fn new(
        entity_id: u32,
        position: Vec3Fix,
        velocity: Vec3Fix,
        heading: QuantizedYaw,
    ) -> Self {
        Self {
            position,
            velocity,
            entity_id,
            heading,
            flags: 0,
            health: 100,
            max_health: 100,
            generation: 0,
            _reserved: [0; 4],
        }
    }

    /// Constructs a new 64-byte aligned entity block with explicit health attributes.
    pub fn with_health(
        entity_id: u32,
        position: Vec3Fix,
        velocity: Vec3Fix,
        heading: QuantizedYaw,
        health: u16,
        max_health: u16,
    ) -> Self {
        Self {
            position,
            velocity,
            entity_id,
            heading,
            flags: 0,
            health,
            max_health,
            generation: 0,
            _reserved: [0; 4],
        }
    }

    /// Extrapolates position along velocity vector for the given time step.
    #[inline(always)]
    pub fn step(&mut self, dt: Fixed64) {
        self.position += self.velocity * dt;
    }

    /// Extrapolates position and velocity using 2nd-order acceleration over the time step.
    #[inline(always)]
    pub fn step_2nd_order(&mut self, acceleration: Vec3Fix, dt: Fixed64) {
        let half_dt_sq = dt * dt * Fixed64::HALF;
        self.position += (self.velocity * dt) + (acceleration * half_dt_sq);
        self.velocity += acceleration * dt;
    }
}

/// 8-lane SIMD-aligned contiguous Struct-of-Arrays chunk for 8 entities.
///
/// Contains parallel 8-element coordinate and kinematic arrays, each strictly aligned
/// to 64 bytes to eliminate false sharing and maximize L1/L2 streaming bandwidth.
#[repr(C, align(64))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlignedSoAChunk8 {
    /// 8-lane X coordinates (64 bytes).
    pub pos_x: [Fixed64; 8],
    /// 8-lane Y coordinates (64 bytes).
    pub pos_y: [Fixed64; 8],
    /// 8-lane Z coordinates (64 bytes).
    pub pos_z: [Fixed64; 8],
    /// 8-lane X velocities (64 bytes).
    pub vel_x: [Fixed64; 8],
    /// 8-lane Y velocities (64 bytes).
    pub vel_y: [Fixed64; 8],
    /// 8-lane Z velocities (64 bytes).
    pub vel_z: [Fixed64; 8],
    /// 8-lane X accelerations (64 bytes).
    pub accel_x: [Fixed64; 8],
    /// 8-lane Y accelerations (64 bytes).
    pub accel_y: [Fixed64; 8],
    /// 8-lane Z accelerations (64 bytes).
    pub accel_z: [Fixed64; 8],
    /// 8-lane entity IDs (32 bytes).
    pub ids: [u32; 8],
    /// 8-lane headings (8 bytes).
    pub headings: [u8; 8],
    /// 8-lane flags (8 bytes).
    pub flags: [u8; 8],
    /// 8-lane health (16 bytes).
    pub health: [u16; 8],
    /// Active entity occupancy count (up to 8).
    pub count: u8,
    /// Active entity bitmask (bit i = 1 if slot i is active).
    pub active_mask: u8,
    /// Reserved padding ensuring 64-byte alignment of chunk control fields.
    pub _reserved: [u8; 62],
}

// Compile-time assertions verifying 64-byte alignment and total block size.
const _: () = assert!(core::mem::size_of::<AlignedSoAChunk8>() == 704);
const _: () = assert!(core::mem::align_of::<AlignedSoAChunk8>() == 64);

impl Default for AlignedSoAChunk8 {
    fn default() -> Self {
        Self::new()
    }
}

impl AlignedSoAChunk8 {
    /// Constructs a new empty 8-lane aligned chunk.
    pub fn new() -> Self {
        Self {
            pos_x: [Fixed64::ZERO; 8],
            pos_y: [Fixed64::ZERO; 8],
            pos_z: [Fixed64::ZERO; 8],
            vel_x: [Fixed64::ZERO; 8],
            vel_y: [Fixed64::ZERO; 8],
            vel_z: [Fixed64::ZERO; 8],
            accel_x: [Fixed64::ZERO; 8],
            accel_y: [Fixed64::ZERO; 8],
            accel_z: [Fixed64::ZERO; 8],
            ids: [0; 8],
            headings: [0; 8],
            flags: [0; 8],
            health: [0; 8],
            count: 0,
            active_mask: 0,
            _reserved: [0; 62],
        }
    }

    /// Returns the number of active entities in the chunk.
    #[inline]
    pub fn len(&self) -> usize {
        self.count as usize
    }

    /// Returns true if no entities reside in the chunk.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Returns true if all 8 slots are occupied.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.count == 8
    }

    /// Appends an entity to the first available slot in the chunk.
    pub fn push(
        &mut self,
        id: u32,
        pos: Vec3Fix,
        vel: Vec3Fix,
        heading: QuantizedYaw,
        health: u16,
    ) -> Result<usize, WorldError> {
        if self.is_full() {
            return Err(WorldError::ChunkCapacityExceeded);
        }
        let slot = self.count as usize;
        self.pos_x[slot] = pos.x;
        self.pos_y[slot] = pos.y;
        self.pos_z[slot] = pos.z;
        self.vel_x[slot] = vel.x;
        self.vel_y[slot] = vel.y;
        self.vel_z[slot] = vel.z;
        self.accel_x[slot] = Fixed64::ZERO;
        self.accel_y[slot] = Fixed64::ZERO;
        self.accel_z[slot] = Fixed64::ZERO;
        self.ids[slot] = id;
        self.headings[slot] = heading.as_byte();
        self.flags[slot] = 0;
        self.health[slot] = health;
        self.active_mask |= 1 << slot;
        self.count += 1;
        Ok(slot)
    }

    /// Appends an entity with explicit acceleration vector to the chunk.
    pub fn push_with_accel(
        &mut self,
        id: u32,
        pos: Vec3Fix,
        vel: Vec3Fix,
        accel: Vec3Fix,
        heading: QuantizedYaw,
        health: u16,
    ) -> Result<usize, WorldError> {
        if self.is_full() {
            return Err(WorldError::ChunkCapacityExceeded);
        }
        let slot = self.count as usize;
        self.pos_x[slot] = pos.x;
        self.pos_y[slot] = pos.y;
        self.pos_z[slot] = pos.z;
        self.vel_x[slot] = vel.x;
        self.vel_y[slot] = vel.y;
        self.vel_z[slot] = vel.z;
        self.accel_x[slot] = accel.x;
        self.accel_y[slot] = accel.y;
        self.accel_z[slot] = accel.z;
        self.ids[slot] = id;
        self.headings[slot] = heading.as_byte();
        self.flags[slot] = 0;
        self.health[slot] = health;
        self.active_mask |= 1 << slot;
        self.count += 1;
        Ok(slot)
    }

    /// Removes an entity at the specified slot using O(1) swap-remove.
    pub fn swap_remove(&mut self, slot: usize) -> Option<u32> {
        if slot >= self.len() {
            return None;
        }
        let last_slot = (self.count - 1) as usize;
        let removed_id = self.ids[slot];

        if slot < last_slot {
            self.pos_x[slot] = self.pos_x[last_slot];
            self.pos_y[slot] = self.pos_y[last_slot];
            self.pos_z[slot] = self.pos_z[last_slot];
            self.vel_x[slot] = self.vel_x[last_slot];
            self.vel_y[slot] = self.vel_y[last_slot];
            self.vel_z[slot] = self.vel_z[last_slot];
            self.accel_x[slot] = self.accel_x[last_slot];
            self.accel_y[slot] = self.accel_y[last_slot];
            self.accel_z[slot] = self.accel_z[last_slot];
            self.ids[slot] = self.ids[last_slot];
            self.headings[slot] = self.headings[last_slot];
            self.flags[slot] = self.flags[last_slot];
            self.health[slot] = self.health[last_slot];
        }

        self.count -= 1;
        self.active_mask &= !(1 << last_slot);
        Some(removed_id)
    }

    /// Performs contiguous linear physics stepping for all active slots in the chunk.
    #[inline]
    pub fn step_kinematics(&mut self, dt: Fixed64) {
        let n = self.count as usize;
        for i in 0..n {
            self.pos_x[i] += self.vel_x[i] * dt;
            self.pos_y[i] += self.vel_y[i] * dt;
            self.pos_z[i] += self.vel_z[i] * dt;
        }
    }

    /// Performs 2nd-order quadratic kinematics for all active slots in the chunk.
    #[inline]
    pub fn step_kinematics_2nd_order(&mut self, dt: Fixed64) {
        let half_dt_sq = dt * dt * Fixed64::HALF;
        let n = self.count as usize;
        for i in 0..n {
            self.pos_x[i] += (self.vel_x[i] * dt) + (self.accel_x[i] * half_dt_sq);
            self.pos_y[i] += (self.vel_y[i] * dt) + (self.accel_y[i] * half_dt_sq);
            self.pos_z[i] += (self.vel_z[i] * dt) + (self.accel_z[i] * half_dt_sq);
            self.vel_x[i] += self.accel_x[i] * dt;
            self.vel_y[i] += self.accel_y[i] * dt;
            self.vel_z[i] += self.accel_z[i] * dt;
        }
    }

    /// Returns the entity components at the given slot if active.
    pub fn get_entity(&self, slot: usize) -> Option<(u32, Vec3Fix, Vec3Fix, QuantizedYaw, u16)> {
        if slot >= self.len() {
            return None;
        }
        let id = self.ids[slot];
        let pos = Vec3Fix::new(self.pos_x[slot], self.pos_y[slot], self.pos_z[slot]);
        let vel = Vec3Fix::new(self.vel_x[slot], self.vel_y[slot], self.vel_z[slot]);
        let yaw = QuantizedYaw::from_byte(self.headings[slot]);
        let hp = self.health[slot];
        Some((id, pos, vel, yaw, hp))
    }
}

/// High-performance entity storage packing active entities into 64-byte cache-line aligned blocks.
///
/// Guarantees sequential L1 data cache prefetching with 0% false sharing during hot 20 Hz simulation loops.
#[derive(Debug, Clone)]
pub struct AlignedBlockStorage {
    /// Contiguous dense array of 64-byte aligned blocks.
    blocks: Vec<AlignedEntityBlock64>,
    /// Sparse ID to dense array index mapping.
    sparse_to_dense: Vec<u32>,
    /// Decoupled cold entity metadata.
    cold_data: Vec<ColdEntityMetadata>,
}

impl AlignedBlockStorage {
    /// Constructs a new aligned block storage with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            blocks: Vec::with_capacity(capacity),
            sparse_to_dense: Vec::new(),
            cold_data: Vec::with_capacity(capacity),
        }
    }

    /// Returns the number of active entities in storage.
    #[inline]
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Returns true if no entities are currently stored.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Returns allocated block capacity.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.blocks.capacity()
    }

    /// Spawns a new entity as an aligned 64-byte block.
    pub fn spawn(
        &mut self,
        block: AlignedEntityBlock64,
        cold: ColdEntityMetadata,
    ) -> Result<usize, WorldError> {
        let id_idx = block.entity_id as usize;
        if id_idx < self.sparse_to_dense.len() && self.sparse_to_dense[id_idx] != SPARSE_SENTINEL {
            return Err(WorldError::EntityAlreadyExists(block.entity_id));
        }

        if id_idx >= self.sparse_to_dense.len() {
            self.sparse_to_dense.resize(id_idx + 1, SPARSE_SENTINEL);
        }

        let dense_idx = self.blocks.len();
        self.blocks.push(block);
        self.cold_data.push(cold);
        self.sparse_to_dense[id_idx] = dense_idx as u32;

        Ok(dense_idx)
    }

    /// Despawns an entity in O(1) time via swap-remove.
    pub fn despawn(&mut self, id: u32) -> Result<ColdEntityMetadata, WorldError> {
        let id_idx = id as usize;
        let dense_idx = if id_idx < self.sparse_to_dense.len() {
            let idx = self.sparse_to_dense[id_idx];
            if idx == SPARSE_SENTINEL {
                return Err(WorldError::EntityNotFound(id));
            }
            idx as usize
        } else {
            return Err(WorldError::EntityNotFound(id));
        };

        let last_idx = self.blocks.len().saturating_sub(1);
        let _removed_block = self.blocks.swap_remove(dense_idx);
        let removed_cold = self.cold_data.swap_remove(dense_idx);

        self.sparse_to_dense[id as usize] = SPARSE_SENTINEL;

        if dense_idx < last_idx {
            let swapped_id = self.blocks[dense_idx].entity_id as usize;
            self.sparse_to_dense[swapped_id] = dense_idx as u32;
        }

        Ok(removed_cold)
    }

    /// Returns a reference to an entity's 64-byte block.
    #[inline]
    pub fn get_block(&self, id: u32) -> Option<&AlignedEntityBlock64> {
        let id_idx = id as usize;
        if id_idx < self.sparse_to_dense.len() {
            let dense_idx = self.sparse_to_dense[id_idx];
            if dense_idx != SPARSE_SENTINEL {
                return self.blocks.get(dense_idx as usize);
            }
        }
        None
    }

    /// Returns a mutable reference to an entity's 64-byte block.
    #[inline]
    pub fn get_block_mut(&mut self, id: u32) -> Option<&mut AlignedEntityBlock64> {
        let id_idx = id as usize;
        if id_idx < self.sparse_to_dense.len() {
            let dense_idx = self.sparse_to_dense[id_idx];
            if dense_idx != SPARSE_SENTINEL {
                return self.blocks.get_mut(dense_idx as usize);
            }
        }
        None
    }

    /// Returns an immutable slice of all aligned entity blocks.
    #[inline]
    pub fn blocks(&self) -> &[AlignedEntityBlock64] {
        &self.blocks
    }

    /// Returns a mutable slice of all aligned entity blocks.
    #[inline]
    pub fn blocks_mut(&mut self) -> &mut [AlignedEntityBlock64] {
        &mut self.blocks
    }

    /// Performs contiguous linear physics extrapolation across all active blocks.
    ///
    /// Iterates sequentially over 64-byte aligned blocks, loading each cache line cleanly.
    pub fn step_kinematics(&mut self, dt: Fixed64) {
        for block in &mut self.blocks {
            block.step(dt);
        }
    }

    /// Performs 2nd-order kinematic stepping given a parallel slice of accelerations.
    pub fn step_kinematics_2nd_order(&mut self, accelerations: &[Vec3Fix], dt: Fixed64) {
        let count = self.blocks.len().min(accelerations.len());
        for (i, &accel) in accelerations.iter().enumerate().take(count) {
            self.blocks[i].step_2nd_order(accel, dt);
        }
    }

    /// Converts this aligned block storage into standard SoaEntityStorage.
    pub fn to_soa_storage(&self) -> SoaEntityStorage {
        let mut soa = SoaEntityStorage::with_capacity(self.blocks.len());
        for (i, block) in self.blocks.iter().enumerate() {
            let cold = self.cold_data.get(i).cloned().unwrap_or_default();
            let params = EntitySpawnParams {
                id: block.entity_id,
                position: block.position,
                velocity: block.velocity,
                acceleration: Vec3Fix::ZERO,
                heading: block.heading,
                flags: block.flags,
                health: block.health as u32,
                max_health: block.max_health as u32,
                cold,
            };
            let _ = soa.spawn(params);
        }
        soa
    }

    /// Constructs aligned block storage from existing SoaEntityStorage.
    pub fn from_soa_storage(storage: &SoaEntityStorage) -> Self {
        let count = storage.len();
        let mut aligned = Self::with_capacity(count);
        let (ids, positions, velocities, headings, flags) = storage.components();
        for i in 0..count {
            let id = ids[i];
            let cold = storage.get_cold_data(id).cloned().unwrap_or_default();
            let (hp, mhp) = storage.get_health(id).unwrap_or((100, 100));
            let block = AlignedEntityBlock64 {
                position: positions[i],
                velocity: velocities[i],
                entity_id: id,
                heading: headings[i],
                flags: flags[i],
                health: hp.min(u16::MAX as u32) as u16,
                max_health: mhp.min(u16::MAX as u32) as u16,
                generation: 0,
                _reserved: [0; 4],
            };
            let _ = aligned.spawn(block, cold);
        }
        aligned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soa_storage_spawn_and_transform_lookup() {
        let mut storage = SoaEntityStorage::with_capacity(16);
        assert!(storage.is_empty());

        let pos1 = Vec3Fix::from_f64(10.0, 0.0, 20.0);
        let vel1 = Vec3Fix::from_f64(1.0, 0.0, 0.0);
        let yaw1 = QuantizedYaw::from_degrees(90.0);

        let idx = storage
            .spawn(EntitySpawnParams::new(101, pos1, vel1, yaw1))
            .expect("spawn 101");
        assert_eq!(idx, 0);
        assert_eq!(storage.len(), 1);

        let (ret_pos, ret_vel, ret_yaw) = storage.get_transform(101).expect("get transform 101");
        assert_eq!(ret_pos, pos1);
        assert_eq!(ret_vel, vel1);
        assert_eq!(ret_yaw, yaw1);

        // Duplicate spawn rejection
        assert!(storage
            .spawn(EntitySpawnParams::new(101, pos1, vel1, yaw1))
            .is_err());
    }

    #[test]
    fn test_soa_storage_swap_remove_integrity() {
        let mut storage = SoaEntityStorage::with_capacity(8);

        // Spawn 3 entities: 10, 20, 30
        for id in [10, 20, 30] {
            let pos = Vec3Fix::from_f64(id as f64, 0.0, 0.0);
            storage
                .spawn(EntitySpawnParams::new(
                    id,
                    pos,
                    Vec3Fix::ZERO,
                    QuantizedYaw::from_degrees(0.0),
                ))
                .expect("spawn");
        }
        assert_eq!(storage.len(), 3);

        // Despawn middle entity (20)
        // Entity 30 should be swapped into index 1
        let removed = storage.despawn(20).expect("despawn 20");
        assert_eq!(removed.name, "");
        assert_eq!(storage.len(), 2);

        // Entity 20 must no longer exist
        assert!(storage.get_transform(20).is_none());
        assert!(storage.despawn(20).is_err());

        // Entity 10 and 30 must remain fully accessible
        let (pos10, _, _) = storage.get_transform(10).expect("get 10");
        assert_eq!(pos10.x.to_f64(), 10.0);

        let (pos30, _, _) = storage.get_transform(30).expect("get 30");
        assert_eq!(pos30.x.to_f64(), 30.0);

        // Dense arrays must remain packed
        let (ids, positions, _, _, _) = storage.components();
        assert_eq!(ids, &[10, 30]);
        assert_eq!(positions[0].x.to_f64(), 10.0);
        assert_eq!(positions[1].x.to_f64(), 30.0);
    }

    #[test]
    fn test_soa_storage_step_kinematics() {
        let mut storage = SoaEntityStorage::with_capacity(4);

        let pos = Vec3Fix::from_f64(0.0, 0.0, 0.0);
        let vel = Vec3Fix::from_f64(10.0, 0.0, 20.0); // 10 m/s X, 20 m/s Z

        storage
            .spawn(EntitySpawnParams::new(
                1,
                pos,
                vel,
                QuantizedYaw::from_degrees(0.0),
            ))
            .expect("spawn");

        // Step kinematics by 50ms (dt = 0.05 seconds = 20 Hz tick)
        let dt = Fixed64::from_f64(0.05);
        storage.step_kinematics(dt);

        let (new_pos, _, _) = storage.get_transform(1).expect("get transform");
        assert!((new_pos.x.to_f64() - 0.5).abs() < 1e-6);
        assert!((new_pos.z.to_f64() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_soa_storage_simd_parity_and_tail_handling() {
        // 27 entities: 3 full 8-lane chunks + 3 tail entities
        let entity_count = 27;
        let mut simd_storage = SoaEntityStorage::with_capacity(entity_count);
        let mut scalar_storage = SoaEntityStorage::with_capacity(entity_count);

        for i in 0..entity_count {
            let id = (i + 1) as u32;
            let pos = Vec3Fix::from_f64((i * 10) as f64, (i * 2) as f64, (i * 5) as f64);
            let vel = Vec3Fix::from_f64((i + 1) as f64 * 1.5, 0.5, (i + 1) as f64 * -2.0);
            let yaw = QuantizedYaw::from_degrees((i * 13) as f64);

            let params = EntitySpawnParams::new(id, pos, vel, yaw);
            simd_storage.spawn(params.clone()).expect("spawn simd");
            scalar_storage.spawn(params).expect("spawn scalar");
        }

        let dt = Fixed64::from_f64(0.05);
        // Step 10 ticks (0.5s total simulation)
        for _ in 0..10 {
            simd_storage.step_kinematics(dt);
            scalar_storage.step_kinematics_scalar(dt);
        }

        // Verify exact bit-for-bit parity across all 27 entities
        let (_, simd_positions, _, _, _) = simd_storage.components();
        let (_, scalar_positions, _, _, _) = scalar_storage.components();

        for i in 0..entity_count {
            assert_eq!(
                simd_positions[i].x.raw(),
                scalar_positions[i].x.raw(),
                "Entity {} X coordinate mismatch between SIMD and scalar",
                i + 1
            );
            assert_eq!(
                simd_positions[i].y.raw(),
                scalar_positions[i].y.raw(),
                "Entity {} Y coordinate mismatch between SIMD and scalar",
                i + 1
            );
            assert_eq!(
                simd_positions[i].z.raw(),
                scalar_positions[i].z.raw(),
                "Entity {} Z coordinate mismatch between SIMD and scalar",
                i + 1
            );
        }
    }

    #[test]
    fn test_soa_storage_simd_16x_parity_and_tail_handling() {
        // Spawn 37 entities: 2 full 16-wide chunks (32) + 1 tail (5)
        let mut storage = SoaEntityStorage::with_capacity(64);
        let mut reference = SoaEntityStorage::with_capacity(64);

        for i in 1..=37 {
            let id = i as u32;
            let pos = Vec3Fix::from_f64((i as f64) * 2.5, (i as f64) * 0.1, (i as f64) * -1.5);
            let vel = Vec3Fix::from_f64((i as f64) * 0.2, 0.0, (i as f64) * 0.4);
            let heading = QuantizedYaw::from_degrees((i as f64) * 10.0);

            let params = EntitySpawnParams::new(id, pos, vel, heading);
            storage.spawn(params.clone()).unwrap();
            reference.spawn(params).unwrap();
        }

        let dt = Fixed64::from_f64(0.05);

        // Step SIMD 16x vs scalar
        storage.step_kinematics_simd_16x(dt);
        reference.step_kinematics_scalar(dt);

        assert_eq!(storage.len(), 37);
        for i in 1..=37 {
            let (pos_simd, _, _) = storage.get_transform(i).expect("simd transform");
            let (pos_scalar, _, _) = reference.get_transform(i).expect("scalar transform");
            assert_eq!(
                pos_simd, pos_scalar,
                "Entity {i} position must match between scalar and SIMD 16x"
            );
        }
    }

    #[test]
    fn test_soa_storage_2nd_order_kinematics() {
        let mut storage = SoaEntityStorage::with_capacity(4);

        let pos = Vec3Fix::from_f64(0.0, 0.0, 0.0);
        let vel = Vec3Fix::from_f64(10.0, 0.0, 0.0);
        let accel = Vec3Fix::from_f64(2.0, 0.0, 0.0); // 2 m/s^2 along X
        let heading = QuantizedYaw::from_degrees(90.0);

        let params = EntitySpawnParams::with_acceleration(1, pos, vel, accel, heading);
        storage.spawn(params).expect("spawn with accel");

        // Verify initial state retrieval
        let (p0, v0, a0, y0) = storage
            .get_transform_2nd_order(1)
            .expect("get 2nd order transform");
        assert_eq!(p0, pos);
        assert_eq!(v0, vel);
        assert_eq!(a0, accel);
        assert_eq!(y0, heading);

        // Step 10 ticks (0.5s total at 20 Hz, dt = 0.05s)
        let dt = Fixed64::from_f64(0.05);
        for _ in 0..10 {
            storage.step_kinematics_2nd_order(dt);
        }

        // Analytical kinematics after t = 0.5s:
        // x(t) = x0 + v0 * t + 0.5 * a * t^2 = 0 + 10 * 0.5 + 0.5 * 2 * (0.25) = 5.0 + 0.25 = 5.25m
        // vx(t) = v0 + a * t = 10 + 2 * 0.5 = 11.0 m/s
        let (p1, v1, a1, _) = storage.get_transform_2nd_order(1).expect("get final");
        assert!((p1.x.to_f64() - 5.25).abs() < 1e-4);
        assert!((v1.x.to_f64() - 11.0).abs() < 1e-4);
        assert_eq!(a1, accel);

        // Slice verification
        let (ids, positions, velocities, accelerations, _, _) = storage.components_2nd_order();
        assert_eq!(ids, &[1]);
        assert_eq!(positions[0], p1);
        assert_eq!(velocities[0], v1);
        assert_eq!(accelerations[0], a1);
    }

    #[test]
    fn test_aligned_entity_block_64_layout_and_stepping() {
        assert_eq!(core::mem::size_of::<AlignedEntityBlock64>(), 64);
        assert_eq!(core::mem::align_of::<AlignedEntityBlock64>(), 64);

        let pos = Vec3Fix::from_f64(10.0, 5.0, -20.0);
        let vel = Vec3Fix::from_f64(2.0, 0.0, 4.0);
        let heading = QuantizedYaw::from_degrees(180.0);
        let mut block = AlignedEntityBlock64::with_health(42, pos, vel, heading, 80, 100);

        assert_eq!(block.entity_id, 42);
        assert_eq!(block.health, 80);
        assert_eq!(block.max_health, 100);

        // Linear step dt = 0.05s
        let dt = Fixed64::from_f64(0.05);
        block.step(dt);
        assert!((block.position.x.to_f64() - 10.1).abs() < 1e-4);
        assert!((block.position.z.to_f64() - (-19.8)).abs() < 1e-4);

        // 2nd-order step with accel = (1.0, 0.0, 0.0)
        let accel = Vec3Fix::from_f64(1.0, 0.0, 0.0);
        block.step_2nd_order(accel, dt);
        // pos.x was 10.1, vel.x was 2.0. displacement = 2.0 * 0.05 + 0.5 * 1.0 * 0.0025 = 0.10125
        assert!((block.position.x.to_f64() - (10.1 + 0.10125)).abs() < 1e-4);
        assert!((block.velocity.x.to_f64() - (2.0 + 0.05)).abs() < 1e-4);
    }

    #[test]
    fn test_aligned_soa_chunk_8_simd_layout_and_stepping() {
        assert_eq!(core::mem::size_of::<AlignedSoAChunk8>(), 704);
        assert_eq!(core::mem::align_of::<AlignedSoAChunk8>(), 64);

        let mut chunk = AlignedSoAChunk8::new();
        assert!(chunk.is_empty());
        assert_eq!(chunk.len(), 0);

        // Fill all 8 slots
        for i in 0..8 {
            let id = 100 + i as u32;
            let pos = Vec3Fix::from_f64(i as f64 * 10.0, 0.0, 0.0);
            let vel = Vec3Fix::from_f64(1.0, 0.0, 0.0);
            let yaw = QuantizedYaw::from_degrees(i as f64 * 45.0);
            let slot = chunk.push(id, pos, vel, yaw, 100).expect("push slot");
            assert_eq!(slot, i);
        }
        assert!(chunk.is_full());
        assert_eq!(chunk.len(), 8);

        // Attempt pushing beyond capacity
        assert!(chunk
            .push(
                999,
                Vec3Fix::ZERO,
                Vec3Fix::ZERO,
                QuantizedYaw::from_byte(0),
                100
            )
            .is_err());

        // Step kinematics
        let dt = Fixed64::from_f64(0.05);
        chunk.step_kinematics(dt);

        for i in 0..8 {
            let (id, pos, vel, _, hp) = chunk.get_entity(i).expect("get entity");
            assert_eq!(id, 100 + i as u32);
            assert_eq!(hp, 100);
            assert_eq!(vel.x.to_f64(), 1.0);
            assert!((pos.x.to_f64() - (i as f64 * 10.0 + 0.05)).abs() < 1e-4);
        }

        // Swap remove middle entity (slot 3)
        let removed = chunk.swap_remove(3);
        assert_eq!(removed, Some(103));
        assert_eq!(chunk.len(), 7);
        // Last entity (107) should now be in slot 3
        let (swapped_id, _, _, _, _) = chunk.get_entity(3).expect("swapped entity");
        assert_eq!(swapped_id, 107);
    }

    #[test]
    fn test_aligned_block_storage_lifecycle_and_parity() {
        let mut storage = AlignedBlockStorage::with_capacity(32);
        assert!(storage.is_empty());

        for i in 1..=20 {
            let id = i as u32;
            let pos = Vec3Fix::from_f64(i as f64 * 5.0, 1.0, i as f64 * -2.0);
            let vel = Vec3Fix::from_f64(0.5, 0.0, 1.0);
            let heading = QuantizedYaw::from_degrees((i * 15) as f64);
            let block = AlignedEntityBlock64::with_health(id, pos, vel, heading, 90, 100);
            storage
                .spawn(block, ColdEntityMetadata::default())
                .expect("spawn");
        }
        assert_eq!(storage.len(), 20);

        let dt = Fixed64::from_f64(0.05);
        storage.step_kinematics(dt);

        // Convert to SoaEntityStorage and assert parity
        let mut soa = storage.to_soa_storage();
        assert_eq!(soa.len(), 20);

        for i in 1..=20 {
            let id = i as u32;
            let block = storage.get_block(id).expect("aligned block");
            let (pos, vel, yaw) = soa.get_transform(id).expect("soa transform");
            assert_eq!(block.position, pos);
            assert_eq!(block.velocity, vel);
            assert_eq!(block.heading, yaw);
        }

        // Step SoaEntityStorage scalar and update back
        soa.step_kinematics_scalar(dt);
        let blocks = soa.to_aligned_blocks();
        assert_eq!(blocks.len(), 20);

        let roundtrip = AlignedBlockStorage::from_soa_storage(&soa);
        assert_eq!(roundtrip.len(), 20);
        for i in 1..=20 {
            let id = i as u32;
            let blk = roundtrip.get_block(id).expect("roundtrip block");
            let (pos, _, _) = soa.get_transform(id).expect("soa transform");
            assert_eq!(blk.position, pos);
        }

        // Despawn check
        let removed_cold = storage.despawn(10).expect("despawn 10");
        assert_eq!(removed_cold.name, "");
        assert_eq!(storage.len(), 19);
        assert!(storage.get_block(10).is_none());
    }
}
