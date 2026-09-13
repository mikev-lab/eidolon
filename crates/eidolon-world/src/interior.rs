//! Interior cell graph and pocket-dimension housing isolation.
//!
//! Implements Star Wars Galaxies style interior cell pocket-dimensions: buildings present
//! only their exterior shell to the open-world spatial grid, while interior decorative
//! items (up to 3,000 per building) exist exclusively within interior cell graphs.
//! Outside players walking past incur 0 bytes of network traffic and 0 collision checks.

use std::collections::{BTreeMap, HashMap, HashSet};

use eidolon_core::structure::InteriorItemRecord;

use crate::error::WorldError;

/// Maximum number of customized interior decorative items per building.
pub const MAX_INTERIOR_ITEMS_PER_BUILDING: usize = 3000;

/// An isolated interior pocket-dimension instance.
#[derive(Debug, Clone)]
pub struct InteriorCell {
    /// Unique interior cell identifier.
    pub cell_id: u32,
    /// Identifier of the parent building or vehicle in the open world.
    pub parent_building_id: u32,
    /// Account identifier of the building owner.
    pub owner_account_id: u64,
    /// Customized interior items indexed by item instance ID.
    pub items: BTreeMap<u32, InteriorItemRecord>,
    /// Player entity IDs currently occupying this interior room.
    pub occupants: HashSet<u32>,
}

impl InteriorCell {
    /// Creates a new interior cell with empty items and occupants.
    pub fn new(cell_id: u32, parent_building_id: u32, owner_account_id: u64) -> Self {
        Self {
            cell_id,
            parent_building_id,
            owner_account_id,
            items: BTreeMap::new(),
            occupants: HashSet::new(),
        }
    }

    /// Adds or updates an interior item in the room.
    pub fn place_item(&mut self, record: InteriorItemRecord) -> Result<(), WorldError> {
        if self.items.len() >= MAX_INTERIOR_ITEMS_PER_BUILDING
            && !self.items.contains_key(&record.item_instance_id)
        {
            return Err(WorldError::InteriorCellFull);
        }
        self.items.insert(record.item_instance_id, record);
        Ok(())
    }

    /// Removes an interior item by instance ID.
    pub fn remove_item(&mut self, item_instance_id: u32) -> Result<InteriorItemRecord, WorldError> {
        self.items
            .remove(&item_instance_id)
            .ok_or(WorldError::ItemNotFound(item_instance_id))
    }

    /// Returns the total number of items customized in this room.
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Serializes all interior item records into a contiguous binary scene manifest.
    ///
    /// Writes 16 bytes per item into caller-provided `out_buffer`.
    /// Returns the total number of bytes written.
    pub fn serialize_manifest(&self, out_buffer: &mut [u8]) -> Result<usize, &'static str> {
        let required_bytes = self.items.len() * 16;
        if out_buffer.len() < required_bytes {
            return Err("Buffer too small for interior scene manifest");
        }

        let mut offset = 0;
        for item in self.items.values() {
            item.write_to(&mut out_buffer[offset..offset + 16])?;
            offset += 16;
        }

        Ok(offset)
    }
}

/// Central manager coordinating all housing and vehicle interior pocket-dimensions.
#[derive(Debug, Default)]
pub struct InteriorCellManager {
    cells: HashMap<u32, InteriorCell>,
    entity_locations: HashMap<u32, u32>, // entity_id -> cell_id
    next_cell_id: u32,
}

impl InteriorCellManager {
    /// Creates a new interior cell manager.
    pub fn new() -> Self {
        Self {
            cells: HashMap::new(),
            entity_locations: HashMap::new(),
            next_cell_id: 1,
        }
    }

    /// Allocates and initializes a new interior cell for a building.
    pub fn create_cell(&mut self, parent_building_id: u32, owner_account_id: u64) -> u32 {
        let cell_id = self.next_cell_id;
        self.next_cell_id = self.next_cell_id.wrapping_add(1);

        let cell = InteriorCell::new(cell_id, parent_building_id, owner_account_id);
        self.cells.insert(cell_id, cell);
        cell_id
    }

    /// Destroys an interior cell, evicting any remaining occupants.
    pub fn destroy_cell(&mut self, cell_id: u32) -> Result<(), WorldError> {
        if let Some(cell) = self.cells.remove(&cell_id) {
            for occ in cell.occupants {
                self.entity_locations.remove(&occ);
            }
            Ok(())
        } else {
            Err(WorldError::InteriorCellNotFound(cell_id))
        }
    }

    /// Adds an interior decorative item into a cell.
    pub fn place_item(
        &mut self,
        cell_id: u32,
        record: InteriorItemRecord,
    ) -> Result<(), WorldError> {
        let cell = self
            .cells
            .get_mut(&cell_id)
            .ok_or(WorldError::InteriorCellNotFound(cell_id))?;
        cell.place_item(record)
    }

    /// Removes an interior decorative item from a cell.
    pub fn remove_item(
        &mut self,
        cell_id: u32,
        item_instance_id: u32,
    ) -> Result<InteriorItemRecord, WorldError> {
        let cell = self
            .cells
            .get_mut(&cell_id)
            .ok_or(WorldError::InteriorCellNotFound(cell_id))?;
        cell.remove_item(item_instance_id)
    }

    /// Transitions a player entity into an interior cell pocket-dimension.
    pub fn enter_cell(&mut self, entity_id: u32, cell_id: u32) -> Result<(), WorldError> {
        if !self.cells.contains_key(&cell_id) {
            return Err(WorldError::InteriorCellNotFound(cell_id));
        }

        // If player was in another cell, evict first
        if let Some(old_cell_id) = self.entity_locations.remove(&entity_id) {
            if let Some(old_cell) = self.cells.get_mut(&old_cell_id) {
                old_cell.occupants.remove(&entity_id);
            }
        }

        let cell = self
            .cells
            .get_mut(&cell_id)
            .ok_or(WorldError::InteriorCellNotFound(cell_id))?;

        cell.occupants.insert(entity_id);
        self.entity_locations.insert(entity_id, cell_id);
        Ok(())
    }

    /// Transitions a player entity out of an interior cell back to open-world space.
    pub fn exit_cell(&mut self, entity_id: u32) -> Result<u32, WorldError> {
        let cell_id = self
            .entity_locations
            .remove(&entity_id)
            .ok_or(WorldError::EntityNotFound(entity_id))?;

        if let Some(cell) = self.cells.get_mut(&cell_id) {
            cell.occupants.remove(&entity_id);
        }

        Ok(cell_id)
    }

    /// Returns the cell ID the entity is currently occupying, if any.
    pub fn get_entity_cell(&self, entity_id: u32) -> Option<u32> {
        self.entity_locations.get(&entity_id).copied()
    }

    /// Returns true if the entity is currently inside an interior pocket dimension.
    pub fn is_inside_interior(&self, entity_id: u32) -> bool {
        self.entity_locations.contains_key(&entity_id)
    }

    /// Returns immutable reference to an interior cell by ID.
    pub fn get_cell(&self, cell_id: u32) -> Option<&InteriorCell> {
        self.cells.get(&cell_id)
    }

    /// Returns the total number of customized items in the specified cell.
    pub fn item_count(&self, cell_id: u32) -> usize {
        self.cells
            .get(&cell_id)
            .map(|c| c.item_count())
            .unwrap_or(0)
    }

    /// Serializes an interior room scene manifest into caller-provided buffer.
    pub fn build_scene_manifest(
        &self,
        cell_id: u32,
        out_buffer: &mut [u8],
    ) -> Result<usize, WorldError> {
        let cell = self
            .cells
            .get(&cell_id)
            .ok_or(WorldError::InteriorCellNotFound(cell_id))?;
        cell.serialize_manifest(out_buffer)
            .map_err(|_| WorldError::SnapshotCorrupted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interior_cell_lifecycle_and_pocket_dimension_isolation() {
        let mut mgr = InteriorCellManager::new();
        let cell_id = mgr.create_cell(101, 5555);

        // Outside player (entity 1) is not inside
        assert!(!mgr.is_inside_interior(1));
        assert_eq!(mgr.get_entity_cell(1), None);

        // Add 3 decorative items into interior cell
        for i in 1..=3 {
            let item = InteriorItemRecord {
                item_instance_id: i,
                item_type_id: 100 + i,
                local_x_mm: (i as i16) * 500,
                local_y_mm: 0,
                local_z_mm: (i as i16) * 1000,
                local_yaw: 0,
                flags: 0,
            };
            mgr.place_item(cell_id, item).expect("Place item");
        }

        assert_eq!(mgr.item_count(cell_id), 3);

        // Outside players walking by receive 0 items (isolation)
        // Only when player enters the cell do they get access to interior state
        mgr.enter_cell(1, cell_id).expect("Enter cell");
        assert!(mgr.is_inside_interior(1));
        assert_eq!(mgr.get_entity_cell(1), Some(cell_id));

        // Serialize scene manifest for entering player (3 items * 16 bytes = 48 bytes)
        let mut manifest_buf = [0u8; 128];
        let bytes_written = mgr
            .build_scene_manifest(cell_id, &mut manifest_buf)
            .expect("Build manifest");
        assert_eq!(bytes_written, 48);

        // Verify decoded item records
        let item1 = InteriorItemRecord::read_from(&manifest_buf[0..16]).expect("Decode item 1");
        assert_eq!(item1.item_instance_id, 1);
        assert_eq!(item1.item_type_id, 101);

        // Player exits cell
        let exited = mgr.exit_cell(1).expect("Exit cell");
        assert_eq!(exited, cell_id);
        assert!(!mgr.is_inside_interior(1));
    }

    #[test]
    fn test_interior_cell_capacity_limit() {
        let mut cell = InteriorCell::new(1, 10, 100);

        // Test boundary capacity (mock filling to max items)
        for i in 0..MAX_INTERIOR_ITEMS_PER_BUILDING {
            let item = InteriorItemRecord {
                item_instance_id: i as u32,
                item_type_id: 1,
                local_x_mm: 0,
                local_y_mm: 0,
                local_z_mm: 0,
                local_yaw: 0,
                flags: 0,
            };
            assert!(cell.place_item(item).is_ok());
        }

        assert_eq!(cell.item_count(), MAX_INTERIOR_ITEMS_PER_BUILDING);

        // 3001st item should be rejected with InteriorCellFull
        let overflow_item = InteriorItemRecord {
            item_instance_id: 999999,
            item_type_id: 1,
            local_x_mm: 0,
            local_y_mm: 0,
            local_z_mm: 0,
            local_yaw: 0,
            flags: 0,
        };
        assert_eq!(
            cell.place_item(overflow_item),
            Err(WorldError::InteriorCellFull)
        );
    }
}
