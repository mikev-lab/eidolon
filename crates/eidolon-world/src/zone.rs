//! Seamless persistent open-world zones, 16-meter boundary seams, and atomic entity handoffs.
//!
//! Enforces zero-loading-screen zone transitions with atomic in-memory migration
//! across contiguous spatial hash grids.

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_spatial::grid::SpatialHashGrid;

use crate::error::WorldError;
pub use crate::error::ZoneId;

/// Orientation axis of an overlapping zone boundary seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeamAxis {
    /// Seam runs along the Z axis (entity transitions across X).
    EastWest,
    /// Seam runs along the X axis (entity transitions across Z).
    NorthSouth,
}

/// Bounding volume and overlapping boundary seam for an open-world zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneBounds {
    /// Minimum horizontal X coordinate.
    pub min_x: Fixed64,
    /// Maximum horizontal X coordinate.
    pub max_x: Fixed64,
    /// Minimum horizontal Z coordinate.
    pub min_z: Fixed64,
    /// Maximum horizontal Z coordinate.
    pub max_z: Fixed64,
    /// Orientation axis of the boundary seam.
    pub seam_axis: SeamAxis,
    /// Lower boundary coordinate of the 16-meter seam.
    pub seam_min: Fixed64,
    /// Upper boundary coordinate of the 16-meter seam.
    pub seam_max: Fixed64,
}

impl ZoneBounds {
    /// Creates a new rectangular zone bounding volume with a 16-meter overlapping seam.
    pub fn new(
        min_x: Fixed64,
        max_x: Fixed64,
        min_z: Fixed64,
        max_z: Fixed64,
        seam_axis: SeamAxis,
        seam_min: Fixed64,
        seam_max: Fixed64,
    ) -> Self {
        Self {
            min_x,
            max_x,
            min_z,
            max_z,
            seam_axis,
            seam_min,
            seam_max,
        }
    }

    /// Returns true if continuous position is within the zone's spatial bounds.
    #[inline]
    pub fn contains(&self, pos: Vec3Fix) -> bool {
        pos.x >= self.min_x && pos.x <= self.max_x && pos.z >= self.min_z && pos.z <= self.max_z
    }

    /// Returns true if position falls within the 16-meter overlapping boundary seam.
    #[inline]
    pub fn in_seam(&self, pos: Vec3Fix) -> bool {
        match self.seam_axis {
            SeamAxis::EastWest => pos.x >= self.seam_min && pos.x <= self.seam_max,
            SeamAxis::NorthSouth => pos.z >= self.seam_min && pos.z <= self.seam_max,
        }
    }

    /// Computes the midpoint coordinate of the 16-meter seam.
    #[inline]
    pub fn seam_midpoint(&self) -> Fixed64 {
        Fixed64::from_raw((self.seam_min.raw() + self.seam_max.raw()) >> 1)
    }

    /// Checks if entity has crossed the seam midpoint plane towards the neighbor zone.
    #[inline]
    pub fn is_past_midpoint(&self, pos: Vec3Fix, towards_positive: bool) -> bool {
        let mid = self.seam_midpoint();
        let coord = match self.seam_axis {
            SeamAxis::EastWest => pos.x,
            SeamAxis::NorthSouth => pos.z,
        };

        if towards_positive {
            coord >= mid
        } else {
            coord <= mid
        }
    }
}

/// Entity state descriptor generated during atomic zone boundary handoffs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationTicket {
    /// Unique entity identifier.
    pub entity_id: u32,
    /// Originating zone identifier.
    pub from_zone: ZoneId,
    /// Target zone identifier.
    pub to_zone: ZoneId,
    /// Authoritative position at the moment of migration.
    pub position: Vec3Fix,
    /// Indicates whether entity is currently inside the 16m overlapping seam.
    pub in_seam: bool,
}

/// Persistent open-world zone encapsulating an isolated spatial hash grid and entity set.
#[derive(Debug)]
pub struct WorldZone {
    /// Unique zone identifier.
    pub id: ZoneId,
    /// Spatial boundaries and seam configuration.
    pub bounds: ZoneBounds,
    /// Adjacent neighbor zone identifier across the primary seam.
    pub neighbor_zone: Option<ZoneId>,
    /// Indicates if migration towards neighbor moves in the positive coordinate direction.
    pub neighbor_is_positive: bool,
    /// Independent spatial hash grid for local entity partitioning.
    pub spatial_grid: SpatialHashGrid,
}

impl WorldZone {
    /// Creates a new open-world zone with pre-allocated spatial capacity.
    pub fn new(
        id: ZoneId,
        bounds: ZoneBounds,
        neighbor_zone: Option<ZoneId>,
        neighbor_is_positive: bool,
        max_entities: usize,
    ) -> Self {
        Self {
            id,
            bounds,
            neighbor_zone,
            neighbor_is_positive,
            spatial_grid: SpatialHashGrid::new(max_entities),
        }
    }

    /// Inserts an entity into the zone's spatial grid.
    pub fn insert_entity(&mut self, entity_id: u32, pos: Vec3Fix) -> Result<(), WorldError> {
        self.spatial_grid
            .insert(entity_id, pos)
            .map_err(|_| WorldError::ZoneFull(self.id))
    }

    /// Removes an entity from the zone's spatial grid.
    pub fn remove_entity(&mut self, entity_id: u32) -> Result<(), WorldError> {
        self.spatial_grid
            .remove(entity_id)
            .map_err(|_| WorldError::EntityNotFound(entity_id))
    }

    /// Updates entity position within the zone's spatial grid.
    ///
    /// Returns true if the entity moved to a different spatial cell bucket.
    pub fn update_position(&mut self, entity_id: u32, pos: Vec3Fix) -> Result<bool, WorldError> {
        self.spatial_grid
            .update_position(entity_id, pos)
            .map_err(|_| WorldError::EntityNotFound(entity_id))
    }

    /// Checks if the entity is currently tracked in this zone.
    #[inline]
    pub fn contains_entity(&self, entity_id: u32) -> bool {
        self.spatial_grid.contains(entity_id)
    }

    /// Returns current position of the entity, or None if not present.
    #[inline]
    pub fn entity_position(&self, entity_id: u32) -> Option<Vec3Fix> {
        self.spatial_grid.get_position(entity_id)
    }

    /// Returns the number of active entities in the zone.
    #[inline]
    pub fn entity_count(&self) -> usize {
        self.spatial_grid.active_count()
    }
}

/// Multi-zone coordinator handling atomic entity migrations across zone seams.
#[derive(Debug, Default)]
pub struct WorldManager {
    zones: Vec<WorldZone>,
}

impl WorldManager {
    /// Creates a new empty `WorldManager`.
    pub fn new() -> Self {
        Self { zones: Vec::new() }
    }

    /// Registers a new zone into the world topology.
    pub fn add_zone(&mut self, zone: WorldZone) {
        self.zones.push(zone);
    }

    /// Returns an immutable reference to the requested zone.
    pub fn get_zone(&self, id: ZoneId) -> Option<&WorldZone> {
        self.zones.iter().find(|z| z.id == id)
    }

    /// Returns a mutable reference to the requested zone.
    pub fn get_zone_mut(&mut self, id: ZoneId) -> Option<&mut WorldZone> {
        self.zones.iter_mut().find(|z| z.id == id)
    }

    /// Ticks entity movement and executes atomic migration if seam midpoint is crossed.
    ///
    /// Executes entirely in-memory with zero heap allocations during the tick loop.
    pub fn tick_entity_movement(
        &mut self,
        entity_id: u32,
        current_zone_id: ZoneId,
        new_pos: Vec3Fix,
    ) -> Result<Option<MigrationTicket>, WorldError> {
        // Step 1: Check if entity crosses boundary towards neighbor
        let (neighbor_id, in_seam) = {
            let current_zone = self
                .get_zone(current_zone_id)
                .ok_or(WorldError::ZoneNotFound(current_zone_id))?;

            let in_seam = current_zone.bounds.in_seam(new_pos);
            let should_migrate = current_zone.neighbor_zone.filter(|_| {
                in_seam
                    && current_zone
                        .bounds
                        .is_past_midpoint(new_pos, current_zone.neighbor_is_positive)
            });

            (should_migrate, in_seam)
        };

        if let Some(to_zone_id) = neighbor_id {
            // Step 2: Atomic in-memory migration
            // Remove from source zone
            {
                let current_zone = self
                    .get_zone_mut(current_zone_id)
                    .ok_or(WorldError::ZoneNotFound(current_zone_id))?;
                current_zone.remove_entity(entity_id)?;
            }

            // Insert into destination zone
            {
                let target_zone = self
                    .get_zone_mut(to_zone_id)
                    .ok_or(WorldError::ZoneNotFound(to_zone_id))?;
                target_zone.insert_entity(entity_id, new_pos)?;
            }

            Ok(Some(MigrationTicket {
                entity_id,
                from_zone: current_zone_id,
                to_zone: to_zone_id,
                position: new_pos,
                in_seam,
            }))
        } else {
            // Normal in-zone position update
            let current_zone = self
                .get_zone_mut(current_zone_id)
                .ok_or(WorldError::ZoneNotFound(current_zone_id))?;
            current_zone.update_position(entity_id, new_pos)?;

            if in_seam {
                // Entity is in seam but hasn't crossed midpoint: flag dual replication
                Ok(Some(MigrationTicket {
                    entity_id,
                    from_zone: current_zone_id,
                    to_zone: current_zone_id,
                    position: new_pos,
                    in_seam: true,
                }))
            } else {
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zone_bounds_and_seam_detection() {
        // Zone A: X in [0, 100], Seam along East edge X in [84, 100] (16m wide)
        let bounds = ZoneBounds::new(
            Fixed64::from_i32(0),
            Fixed64::from_i32(100),
            Fixed64::from_i32(0),
            Fixed64::from_i32(100),
            SeamAxis::EastWest,
            Fixed64::from_i32(84),
            Fixed64::from_i32(100),
        );

        assert_eq!(bounds.seam_midpoint(), Fixed64::from_i32(92));

        // Inside zone, outside seam
        let p1 = Vec3Fix::from_i32(50, 0, 50);
        assert!(bounds.contains(p1));
        assert!(!bounds.in_seam(p1));

        // Inside seam, before midpoint
        let p2 = Vec3Fix::from_i32(88, 0, 50);
        assert!(bounds.contains(p2));
        assert!(bounds.in_seam(p2));
        assert!(!bounds.is_past_midpoint(p2, true));

        // Inside seam, past midpoint (>= 92)
        let p3 = Vec3Fix::from_i32(94, 0, 50);
        assert!(bounds.contains(p3));
        assert!(bounds.in_seam(p3));
        assert!(bounds.is_past_midpoint(p3, true));
    }

    #[test]
    fn test_world_manager_atomic_migration() {
        let mut manager = WorldManager::new();

        // Zone 1: X in [0, 100], seam at [84, 100], neighbor = Zone 2 (positive X)
        let bounds1 = ZoneBounds::new(
            Fixed64::from_i32(0),
            Fixed64::from_i32(100),
            Fixed64::from_i32(0),
            Fixed64::from_i32(100),
            SeamAxis::EastWest,
            Fixed64::from_i32(84),
            Fixed64::from_i32(100),
        );
        let mut zone1 = WorldZone::new(ZoneId(1), bounds1, Some(ZoneId(2)), true, 64);

        // Zone 2: X in [84, 200], seam at [84, 100], neighbor = Zone 1 (negative X)
        let bounds2 = ZoneBounds::new(
            Fixed64::from_i32(84),
            Fixed64::from_i32(200),
            Fixed64::from_i32(0),
            Fixed64::from_i32(100),
            SeamAxis::EastWest,
            Fixed64::from_i32(84),
            Fixed64::from_i32(100),
        );
        let zone2 = WorldZone::new(ZoneId(2), bounds2, Some(ZoneId(1)), false, 64);

        // Entity 5 starts in Zone 1
        zone1
            .insert_entity(5, Vec3Fix::from_i32(50, 0, 50))
            .expect("insert entity 5");

        manager.add_zone(zone1);
        manager.add_zone(zone2);

        // Move entity towards seam (X = 86): in seam, but not past midpoint (92)
        let ticket1 = manager
            .tick_entity_movement(5, ZoneId(1), Vec3Fix::from_i32(86, 0, 50))
            .expect("tick 1");
        assert!(ticket1.is_some());
        let t1 = ticket1.unwrap();
        assert_eq!(t1.from_zone, ZoneId(1));
        assert_eq!(t1.to_zone, ZoneId(1));
        assert!(t1.in_seam);

        // Move entity past midpoint (X = 95): triggers atomic migration from Zone 1 to Zone 2
        let ticket2 = manager
            .tick_entity_movement(5, ZoneId(1), Vec3Fix::from_i32(95, 0, 50))
            .expect("tick 2");
        assert!(ticket2.is_some());
        let t2 = ticket2.unwrap();
        assert_eq!(t2.from_zone, ZoneId(1));
        assert_eq!(t2.to_zone, ZoneId(2));

        // Entity 5 is now in Zone 2, removed from Zone 1
        assert!(!manager.get_zone(ZoneId(1)).unwrap().contains_entity(5));
        assert!(manager.get_zone(ZoneId(2)).unwrap().contains_entity(5));
    }
}
