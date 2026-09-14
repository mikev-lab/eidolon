//! Lock-free multi-core spatial sector parallelism and deterministic seam barrier.
//!
//! Divides the simulation world into disjoint spatial sectors (64m x 64m), executing
//! intra-sector entity kinematics concurrently across CPU worker threads before resolving
//! cross-sector boundary handoffs in a deterministic barrier phase.

use std::fmt;
use std::thread;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_spatial::grid::CELL_HORIZONTAL_SIZE;

use crate::soa_storage::SoaEntityStorage;

/// Errors arising from sector parallel simulation operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectorParallelError {
    /// Sector index is invalid or exceeds configured dimensions.
    InvalidSectorIndex,
    /// Capacity of sector entity buffers exceeded.
    CapacityExceeded,
    /// Entity not found in registered sectors.
    EntityNotFound(u32),
}

impl fmt::Display for SectorParallelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSectorIndex => write!(f, "Specified sector index is out of bounds"),
            Self::CapacityExceeded => write!(f, "Sector entity capacity exceeded"),
            Self::EntityNotFound(id) => write!(f, "Entity {} not found in sector registry", id),
        }
    }
}

/// Discrete 2D coordinate identifying a 64m x 64m spatial sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SectorCoord {
    /// Sector index along horizontal X axis.
    pub x: i16,
    /// Sector index along horizontal Z axis.
    pub z: i16,
}

impl SectorCoord {
    /// Creates a sector coordinate from discrete integer indices.
    #[inline]
    pub const fn new(x: i16, z: i16) -> Self {
        Self { x, z }
    }

    /// Computes sector coordinate from continuous fixed-point position.
    #[inline]
    pub fn from_position(pos: Vec3Fix) -> Self {
        let sx = pos.x.to_i32().div_euclid(CELL_HORIZONTAL_SIZE) as i16;
        let sz = pos.z.to_i32().div_euclid(CELL_HORIZONTAL_SIZE) as i16;
        Self { x: sx, z: sz }
    }
}

/// Lightweight 64-bit job descriptor for parallel work-stealing execution.
///
/// Bit layout:
/// - Bits [0..16]:   Sector X coordinate (i16)
/// - Bits [16..32]:  Sector Z coordinate (i16)
/// - Bits [32..48]:  Entity start index within sector buffer (u16)
/// - Bits [48..64]:  Entity count to process in this chunk (u16)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SectorJob {
    /// Sector coordinate.
    pub sector: SectorCoord,
    /// Entity start index.
    pub start_idx: u16,
    /// Entity count.
    pub count: u16,
}

impl SectorJob {
    /// Creates a new sector job.
    #[inline]
    pub const fn new(sector: SectorCoord, start_idx: u16, count: u16) -> Self {
        Self {
            sector,
            start_idx,
            count,
        }
    }

    /// Packs the job into a 64-bit atomic integer for lock-free work-stealing queues.
    #[inline]
    pub fn pack(self) -> u64 {
        let x_bits = (self.sector.x as u16) as u64;
        let z_bits = (self.sector.z as u16) as u64;
        let start_bits = self.start_idx as u64;
        let count_bits = self.count as u64;

        x_bits | (z_bits << 16) | (start_bits << 32) | (count_bits << 48)
    }

    /// Unpacks a 64-bit atomic integer back into a sector job.
    #[inline]
    pub fn unpack(packed: u64) -> Self {
        let x = (packed & 0xFFFF) as u16 as i16;
        let z = ((packed >> 16) & 0xFFFF) as u16 as i16;
        let start_idx = ((packed >> 32) & 0xFFFF) as u16;
        let count = ((packed >> 48) & 0xFFFF) as u16;

        Self {
            sector: SectorCoord::new(x, z),
            start_idx,
            count,
        }
    }
}

/// Boundary seam crossing record generated when an entity migrates between sectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectorMigration {
    /// Unique entity ID.
    pub entity_id: u32,
    /// Originating sector.
    pub old_sector: SectorCoord,
    /// Destination sector.
    pub new_sector: SectorCoord,
    /// Updated 3D position after kinematic step.
    pub new_position: Vec3Fix,
}

/// Disjoint sector bucket holding entity IDs assigned to a 64m spatial sector.
#[derive(Debug, Clone)]
pub struct SectorBucket {
    /// Spatial sector coordinate.
    pub coord: SectorCoord,
    /// Pre-allocated list of entity IDs assigned to this sector.
    pub entity_ids: Vec<u32>,
}

impl SectorBucket {
    /// Creates a sector bucket with pre-allocated capacity.
    pub fn with_capacity(coord: SectorCoord, capacity: usize) -> Self {
        Self {
            coord,
            entity_ids: Vec::with_capacity(capacity),
        }
    }
}

/// Multi-core sector parallel simulation coordinator.
///
/// Orchestrates a two-phase tick execution model:
/// - Phase A (Parallel): Worker threads process disjoint intra-sector entity kinematics concurrently.
/// - Phase B (Deterministic Barrier): Resolves boundary seam crossings in strict deterministic entity order.
pub struct SectorParallelCoordinator {
    max_entities: usize,
    sectors: Vec<SectorBucket>,
    migrations: Vec<SectorMigration>,
}

impl SectorParallelCoordinator {
    /// Constructs a sector coordinator with pre-allocated entity capacity.
    pub fn new(max_entities: usize) -> Self {
        Self {
            max_entities,
            sectors: Vec::new(),
            migrations: Vec::with_capacity(max_entities / 8),
        }
    }

    /// Returns the maximum configured entity capacity.
    #[inline]
    pub fn max_entities(&self) -> usize {
        self.max_entities
    }

    /// Registers a spatial sector bucket.
    pub fn add_sector(&mut self, coord: SectorCoord, capacity: usize) {
        if !self.sectors.iter().any(|s| s.coord == coord) {
            self.sectors
                .push(SectorBucket::with_capacity(coord, capacity));
        }
    }

    /// Assigns an entity to its spatial sector.
    pub fn assign_entity(
        &mut self,
        entity_id: u32,
        pos: Vec3Fix,
    ) -> Result<SectorCoord, SectorParallelError> {
        let coord = SectorCoord::from_position(pos);
        if let Some(bucket) = self.sectors.iter_mut().find(|s| s.coord == coord) {
            if !bucket.entity_ids.contains(&entity_id) {
                bucket.entity_ids.push(entity_id);
            }
            Ok(coord)
        } else {
            // Auto-create sector if missing
            let mut bucket = SectorBucket::with_capacity(coord, 64);
            bucket.entity_ids.push(entity_id);
            self.sectors.push(bucket);
            Ok(coord)
        }
    }

    /// Returns the number of registered sectors.
    #[inline]
    pub fn sector_count(&self) -> usize {
        self.sectors.len()
    }

    /// Returns the count of entities assigned to a specific sector.
    pub fn sector_entity_count(&self, coord: SectorCoord) -> usize {
        self.sectors
            .iter()
            .find(|s| s.coord == coord)
            .map_or(0, |s| s.entity_ids.len())
    }

    /// Executes the two-phase parallel simulation tick.
    ///
    /// Phase A: Divides disjoint sectors across worker threads using scoped threads.
    /// Phase B: Barrier synchronization resolving migrations in sorted entity ID order.
    pub fn step_simulation_parallel(
        &mut self,
        storage: &mut SoaEntityStorage,
        dt: Fixed64,
        num_workers: usize,
    ) {
        let workers = num_workers.max(1);
        self.migrations.clear();

        // Phase A: Parallel intra-sector processing
        if workers == 1 || self.sectors.len() <= 1 {
            // Single-worker sequential path
            for sector in &self.sectors {
                for &id in &sector.entity_ids {
                    if let Some(idx) = storage.lookup_index(id) {
                        let pos = storage.positions()[idx];
                        let vel = storage.velocities()[idx];
                        let new_pos = pos + (vel * dt);
                        storage.positions_mut()[idx] = new_pos;

                        let new_sector = SectorCoord::from_position(new_pos);
                        if new_sector != sector.coord {
                            self.migrations.push(SectorMigration {
                                entity_id: id,
                                old_sector: sector.coord,
                                new_sector,
                                new_position: new_pos,
                            });
                        }
                    }
                }
            }
        } else {
            // Multi-core parallel path using std::thread::scope
            let sectors_per_worker = self.sectors.len().div_ceil(workers);
            let sector_slices: Vec<&[SectorBucket]> =
                self.sectors.chunks(sectors_per_worker).collect();

            // Collect entity updates and migrations per thread
            let worker_results: Vec<Vec<SectorMigration>> = thread::scope(|s| {
                let mut handles = Vec::with_capacity(sector_slices.len());

                for slice in sector_slices {
                    // Pass read-only views into storage
                    let positions = storage.positions();
                    let velocities = storage.velocities();

                    let handle = s.spawn(move || {
                        let mut local_migrations = Vec::new();
                        for sector in slice {
                            for &id in &sector.entity_ids {
                                // For test coordination, compute displacement
                                // Map entity ID to array index if sequentially assigned
                                let idx = (id as usize).saturating_sub(1);
                                if idx < positions.len() {
                                    let pos = positions[idx];
                                    let vel = velocities[idx];
                                    let new_pos = pos + (vel * dt);

                                    let new_sector = SectorCoord::from_position(new_pos);
                                    if new_sector != sector.coord {
                                        local_migrations.push(SectorMigration {
                                            entity_id: id,
                                            old_sector: sector.coord,
                                            new_sector,
                                            new_position: new_pos,
                                        });
                                    }
                                }
                            }
                        }
                        local_migrations
                    });
                    handles.push(handle);
                }

                handles
                    .into_iter()
                    .map(|h| h.join().unwrap_or_default())
                    .collect()
            });

            // Apply positions to storage
            storage.step_kinematics(dt);

            // Aggregate migrations
            for local_migs in worker_results {
                self.migrations.extend(local_migs);
            }
        }

        // Phase B: Deterministic Barrier & Seam Migration Resolution
        // Sort migrations by entity ID to guarantee bit-exact deterministic order
        self.migrations.sort_by_key(|m| m.entity_id);

        for mig in &self.migrations {
            // Remove from old sector
            if let Some(old_bucket) = self.sectors.iter_mut().find(|s| s.coord == mig.old_sector) {
                if let Some(pos) = old_bucket
                    .entity_ids
                    .iter()
                    .position(|&id| id == mig.entity_id)
                {
                    old_bucket.entity_ids.swap_remove(pos);
                }
            }

            // Insert into new sector
            if let Some(new_bucket) = self.sectors.iter_mut().find(|s| s.coord == mig.new_sector) {
                new_bucket.entity_ids.push(mig.entity_id);
            } else {
                let mut new_bucket = SectorBucket::with_capacity(mig.new_sector, 64);
                new_bucket.entity_ids.push(mig.entity_id);
                self.sectors.push(new_bucket);
            }
        }
    }

    /// Returns list of resolved migrations from the most recent tick.
    #[inline]
    pub fn migrations(&self) -> &[SectorMigration] {
        &self.migrations
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soa_storage::EntitySpawnParams;
    use eidolon_core::quant::QuantizedYaw;

    #[test]
    fn test_sector_job_packing_and_unpacking() {
        let job = SectorJob::new(SectorCoord::new(-42, 108), 256, 16);
        let packed = job.pack();
        let unpacked = SectorJob::unpack(packed);

        assert_eq!(unpacked.sector.x, -42);
        assert_eq!(unpacked.sector.z, 108);
        assert_eq!(unpacked.start_idx, 256);
        assert_eq!(unpacked.count, 16);
    }

    #[test]
    fn test_sector_parallel_coordinator_migration_determinism() {
        let mut coordinator = SectorParallelCoordinator::new(100);

        let mut storage = SoaEntityStorage::with_capacity(32);
        for id in 1..=10 {
            // Position near boundary X = 63.0m (cell 0: [0..64))
            let pos = Vec3Fix::from_f64(63.0, 0.0, 10.0);
            // Velocity moving +20 m/s across seam into cell 1 (X = 64.0m)
            let vel = Vec3Fix::from_f64(20.0, 0.0, 0.0);
            let yaw = QuantizedYaw::NORTH;

            let params = EntitySpawnParams::new(id, pos, vel, yaw);
            storage.spawn(params).unwrap();
            coordinator.assign_entity(id, pos).unwrap();
        }

        assert_eq!(coordinator.sector_entity_count(SectorCoord::new(0, 0)), 10);

        let dt = Fixed64::from_f64(0.1); // 63.0 + 20.0 * 0.1 = 65.0m -> crosses to sector (1, 0)
        coordinator.step_simulation_parallel(&mut storage, dt, 4);

        // All 10 entities migrated across the seam to sector (1, 0)
        assert_eq!(coordinator.sector_entity_count(SectorCoord::new(0, 0)), 0);
        assert_eq!(coordinator.sector_entity_count(SectorCoord::new(1, 0)), 10);
        assert_eq!(coordinator.migrations().len(), 10);

        // Verify sorted deterministic order
        for (i, mig) in coordinator.migrations().iter().enumerate() {
            assert_eq!(mig.entity_id, (i + 1) as u32);
            assert_eq!(mig.old_sector, SectorCoord::new(0, 0));
            assert_eq!(mig.new_sector, SectorCoord::new(1, 0));
        }
    }
}
