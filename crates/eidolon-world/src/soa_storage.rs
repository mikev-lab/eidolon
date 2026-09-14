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
}
