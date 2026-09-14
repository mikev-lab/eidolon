//! Struct-of-Arrays (SoA) world storage and cache-aligned memory layout.
//!
//! Reorganizes active entities into contiguous parallel vectors to maximize L1 data cache
//! utilization during dense 20 Hz tick simulation and physics extrapolation loops.

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::quant::QuantizedYaw;

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
    /// Creates a new spawn parameter bundle with default health and metadata.
    pub fn new(id: u32, position: Vec3Fix, velocity: Vec3Fix, heading: QuantizedYaw) -> Self {
        Self {
            id,
            position,
            velocity,
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
    /// Updates positions: `pos = pos + vel * dt` with 100% cache line utilization and zero branching.
    pub fn step_kinematics(&mut self, dt: Fixed64) {
        let count = self.positions.len();
        for i in 0..count {
            let displacement = self.velocities[i] * dt;
            self.positions[i] += displacement;
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
}
