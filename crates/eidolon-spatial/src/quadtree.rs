//! Micro-hotspot dynamic sub-sharding and quadtree spatial decomposition.
//!
//! Provides hierarchical subdivision of congested 64m spatial cells into 16m
//! micro-quadrants (NW, NE, SW, SE) when entity density exceeds threshold (>250 entities),
//! pruning up to 90% of spatial distance checks during massive player congregations.

use crate::grid::{CellCoord, SpatialError, SpatialQueryResult, CELL_HORIZONTAL_SIZE};
use eidolon_core::fixed::{Fixed64, Vec3Fix};

/// Threshold of entities in a 64m cell triggering dynamic subdivision into micro-quadrants.
pub const HOTSPOT_SPLIT_THRESHOLD: usize = 250;

/// Threshold of entities below which micro-quadrants contract and merge back into a flat root cell.
pub const HOTSPOT_MERGE_THRESHOLD: usize = 100;

/// Horizontal edge size of a micro-quadrant leaf in meters (16 meters).
pub const MICRO_QUADRANT_SIZE: i32 = 16;

/// Horizontal edge size of a depth-1 quadrant in meters (32 meters).
pub const DEPTH1_QUADRANT_SIZE: i32 = 32;

/// Number of micro-quadrants per 64m cell at depth 2 (4x4 = 16 sub-quadrants of 16m).
pub const MICRO_QUADRANTS_PER_CELL: usize = 16;

/// Sentinel index representing an empty child or terminal link in pre-allocated pools.
pub const QUADTREE_EMPTY: u32 = u32::MAX;

/// 2D horizontal Axis-Aligned Bounding Box on the (X, Z) ground plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QuadtreeAabb {
    /// Minimum X coordinate in world space.
    pub min_x: Fixed64,
    /// Maximum X coordinate in world space.
    pub max_x: Fixed64,
    /// Minimum Z coordinate in world space.
    pub min_z: Fixed64,
    /// Maximum Z coordinate in world space.
    pub max_z: Fixed64,
}

impl QuadtreeAabb {
    /// Constructs a new axis-aligned bounding box from world-space bounds.
    #[inline]
    pub const fn new(min_x: Fixed64, max_x: Fixed64, min_z: Fixed64, max_z: Fixed64) -> Self {
        Self {
            min_x,
            max_x,
            min_z,
            max_z,
        }
    }

    /// Computes the squared distance from a 2D point (cx, cz) to the closest point on the AABB.
    /// Returns Fixed64::ZERO if the point is strictly inside the AABB.
    #[inline]
    pub fn distance_squared_to_point(&self, cx: Fixed64, cz: Fixed64) -> Fixed64 {
        let dx = if cx < self.min_x {
            self.min_x - cx
        } else if cx > self.max_x {
            cx - self.max_x
        } else {
            Fixed64::ZERO
        };

        let dz = if cz < self.min_z {
            self.min_z - cz
        } else if cz > self.max_z {
            cz - self.max_z
        } else {
            Fixed64::ZERO
        };

        dx * dx + dz * dz
    }

    /// Returns true if a query circle centered at (cx, cz) with squared radius radius_sq intersects this AABB.
    #[inline]
    pub fn intersects_circle(&self, cx: Fixed64, cz: Fixed64, radius_sq: Fixed64) -> bool {
        self.distance_squared_to_point(cx, cz) <= radius_sq
    }
}

/// Depth-1 quadrant identifiers: NW, NE, SW, SE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Depth1Quadrant {
    /// North-West quadrant (local x in 0..32, local z in 32..64).
    NW = 0,
    /// North-East quadrant (local x in 32..64, local z in 32..64).
    NE = 1,
    /// South-West quadrant (local x in 0..32, local z in 0..32).
    SW = 2,
    /// South-East quadrant (local x in 32..64, local z in 0..32).
    SE = 3,
}

/// State of a single congested 64m spatial cell managed dynamically.
#[derive(Debug, Clone)]
pub struct HotspotCell {
    /// 3D cell coordinate.
    pub cell: CellCoord,
    /// Whether this cell is currently subdivided into micro-quadrants.
    pub is_split: bool,
    /// Total number of active entities in this cell.
    pub entity_count: usize,
    /// Intrusive list head when cell is in flat (unsplit) mode.
    pub flat_head: u32,
    /// Intrusive list heads for the 16 micro-quadrants (depth 2).
    pub leaf_heads: [u32; MICRO_QUADRANTS_PER_CELL],
    /// Entity count in each micro-quadrant leaf.
    pub leaf_counts: [u32; MICRO_QUADRANTS_PER_CELL],
    /// Precomputed AABBs for the 4 depth-1 quadrants (32m each: NW, NE, SW, SE).
    pub quadrant_aabbs: [QuadtreeAabb; 4],
    /// Precomputed AABBs for the 16 micro-quadrant leaves (16m each).
    pub leaf_aabbs: [QuadtreeAabb; MICRO_QUADRANTS_PER_CELL],
}

impl HotspotCell {
    /// Initializes a new hotspot cell with precomputed hierarchical AABB bounding volumes.
    pub fn new(cell: CellCoord) -> Self {
        let base_x = cell.x * CELL_HORIZONTAL_SIZE;
        let base_z = cell.z * CELL_HORIZONTAL_SIZE;

        // Compute 4 depth-1 quadrant AABBs (32m each)
        // 0: NW (x: base..base+32, z: base+32..base+64)
        // 1: NE (x: base+32..base+64, z: base+32..base+64)
        // 2: SW (x: base..base+32, z: base..base+32)
        // 3: SE (x: base+32..base+64, z: base..base+32)
        let q_nw = QuadtreeAabb::new(
            Fixed64::from_i32(base_x),
            Fixed64::from_i32(base_x + 32),
            Fixed64::from_i32(base_z + 32),
            Fixed64::from_i32(base_z + 64),
        );
        let q_ne = QuadtreeAabb::new(
            Fixed64::from_i32(base_x + 32),
            Fixed64::from_i32(base_x + 64),
            Fixed64::from_i32(base_z + 32),
            Fixed64::from_i32(base_z + 64),
        );
        let q_sw = QuadtreeAabb::new(
            Fixed64::from_i32(base_x),
            Fixed64::from_i32(base_x + 32),
            Fixed64::from_i32(base_z),
            Fixed64::from_i32(base_z + 32),
        );
        let q_se = QuadtreeAabb::new(
            Fixed64::from_i32(base_x + 32),
            Fixed64::from_i32(base_x + 64),
            Fixed64::from_i32(base_z),
            Fixed64::from_i32(base_z + 32),
        );
        let quadrant_aabbs = [q_nw, q_ne, q_sw, q_se];

        // Compute 16 micro-quadrant leaf AABBs (16m each)
        // leaf_idx = iz * 4 + ix
        let mut leaf_aabbs = [QuadtreeAabb::default(); MICRO_QUADRANTS_PER_CELL];
        for iz in 0..4 {
            for ix in 0..4 {
                let lx_min = base_x + ix * 16;
                let lx_max = lx_min + 16;
                let lz_min = base_z + iz * 16;
                let lz_max = lz_min + 16;
                let idx = (iz * 4 + ix) as usize;
                leaf_aabbs[idx] = QuadtreeAabb::new(
                    Fixed64::from_i32(lx_min),
                    Fixed64::from_i32(lx_max),
                    Fixed64::from_i32(lz_min),
                    Fixed64::from_i32(lz_max),
                );
            }
        }

        Self {
            cell,
            is_split: false,
            entity_count: 0,
            flat_head: QUADTREE_EMPTY,
            leaf_heads: [QUADTREE_EMPTY; MICRO_QUADRANTS_PER_CELL],
            leaf_counts: [0; MICRO_QUADRANTS_PER_CELL],
            quadrant_aabbs,
            leaf_aabbs,
        }
    }

    /// Computes which of the 16 micro-quadrants a world position falls into.
    #[inline]
    pub fn compute_leaf_index(&self, pos: Vec3Fix) -> usize {
        let base_x = self.cell.x * CELL_HORIZONTAL_SIZE;
        let base_z = self.cell.z * CELL_HORIZONTAL_SIZE;

        let local_x = pos.x.to_i32() - base_x;
        let local_z = pos.z.to_i32() - base_z;

        let ix = (local_x / MICRO_QUADRANT_SIZE).clamp(0, 3) as usize;
        let iz = (local_z / MICRO_QUADRANT_SIZE).clamp(0, 3) as usize;

        iz * 4 + ix
    }

    /// Returns the depth-1 quadrant index (0=NW, 1=NE, 2=SW, 3=SE) for a given leaf index (0..15).
    #[inline]
    pub const fn leaf_to_quadrant(leaf_idx: usize) -> usize {
        let ix = leaf_idx % 4;
        let iz = leaf_idx / 4;

        let qx = ix / 2; // 0 or 1
        let qz = iz / 2; // 0 or 1

        match (qx, qz) {
            (0, 1) => 0, // NW
            (1, 1) => 1, // NE
            (0, 0) => 2, // SW
            _ => 3,      // SE
        }
    }

    /// Returns the four leaf indices corresponding to a depth-1 quadrant index.
    #[inline]
    pub const fn quadrant_to_leaves(quadrant_idx: usize) -> [usize; 4] {
        match quadrant_idx {
            // NW: ix in 0..1, iz in 2..3 => iz*4 + ix => (8, 9, 12, 13)
            0 => [8, 9, 12, 13],
            // NE: ix in 2..3, iz in 2..3 => (10, 11, 14, 15)
            1 => [10, 11, 14, 15],
            // SW: ix in 0..1, iz in 0..1 => (0, 1, 4, 5)
            2 => [0, 1, 4, 5],
            // SE: ix in 2..3, iz in 0..1 => (2, 3, 6, 7)
            _ => [2, 3, 6, 7],
        }
    }
}

/// Dynamic micro-hotspot manager coordinating hierarchical quadtree spatial sub-sharding.
///
/// Pre-allocates all memory pools up-front to guarantee strictly zero heap allocations
/// during hot simulation ticks, even when thousands of entities cluster in a single cell.
#[derive(Debug)]
pub struct HotspotManager {
    max_entities: usize,
    max_hotspots: usize,

    // Pre-allocated hotspot cell pool
    hotspot_cells: Vec<HotspotCell>,
    active_hotspots_count: usize,

    // Intrusive entity links and metadata
    entity_next: Vec<u32>,
    entity_prev: Vec<u32>,
    entity_positions: Vec<Vec3Fix>,
    entity_hotspot_id: Vec<u16>, // 1-based index into hotspot_cells (0 = none)
    entity_leaf_idx: Vec<u8>,    // micro-quadrant index 0..15
    entity_active: Vec<bool>,
}

impl HotspotManager {
    /// Constructs a hotspot manager with pre-allocated capacities.
    pub fn new(max_entities: usize, max_hotspots: usize) -> Self {
        let mut hotspot_cells = Vec::with_capacity(max_hotspots);
        for _ in 0..max_hotspots {
            hotspot_cells.push(HotspotCell::new(CellCoord::default()));
        }

        Self {
            max_entities,
            max_hotspots,
            hotspot_cells,
            active_hotspots_count: 0,
            entity_next: vec![QUADTREE_EMPTY; max_entities],
            entity_prev: vec![QUADTREE_EMPTY; max_entities],
            entity_positions: vec![Vec3Fix::ZERO; max_entities],
            entity_hotspot_id: vec![0; max_entities],
            entity_leaf_idx: vec![0; max_entities],
            entity_active: vec![false; max_entities],
        }
    }

    /// Returns the number of currently active hotspot cells.
    #[inline]
    pub fn active_hotspot_count(&self) -> usize {
        self.active_hotspots_count
    }

    /// Finds the index of an active hotspot cell by its CellCoord, if registered.
    pub fn find_hotspot(&self, cell: CellCoord) -> Option<usize> {
        (0..self.active_hotspots_count).find(|&i| self.hotspot_cells[i].cell == cell)
    }

    /// Returns true if the cell is currently a registered hotspot.
    #[inline]
    pub fn is_hotspot(&self, cell: CellCoord) -> bool {
        self.find_hotspot(cell).is_some()
    }

    /// Returns true if the cell is registered and split into micro-quadrants.
    #[inline]
    pub fn is_split(&self, cell: CellCoord) -> bool {
        if let Some(idx) = self.find_hotspot(cell) {
            self.hotspot_cells[idx].is_split
        } else {
            false
        }
    }

    /// Gets or creates a hotspot tracking entry for a cell.
    fn get_or_create_hotspot(&mut self, cell: CellCoord) -> Result<usize, SpatialError> {
        if let Some(idx) = self.find_hotspot(cell) {
            return Ok(idx);
        }

        if self.active_hotspots_count >= self.max_hotspots {
            return Err(SpatialError::CapacityExceeded);
        }

        let new_idx = self.active_hotspots_count;
        self.hotspot_cells[new_idx] = HotspotCell::new(cell);
        self.active_hotspots_count += 1;
        Ok(new_idx)
    }

    /// Inserts an entity into a cell managed by the hotspot system.
    pub fn insert(&mut self, entity_id: u32, pos: Vec3Fix) -> Result<(), SpatialError> {
        let id = entity_id as usize;
        if id >= self.max_entities {
            return Err(SpatialError::InvalidEntityId(entity_id));
        }
        if self.entity_active[id] {
            return Err(SpatialError::EntityAlreadyExists(entity_id));
        }

        let cell = CellCoord::from_position(pos);
        let h_idx = self.get_or_create_hotspot(cell)?;

        self.entity_positions[id] = pos;
        self.entity_active[id] = true;
        self.entity_hotspot_id[id] = (h_idx + 1) as u16;

        let leaf_idx = self.hotspot_cells[h_idx].compute_leaf_index(pos);
        self.entity_leaf_idx[id] = leaf_idx as u8;

        if self.hotspot_cells[h_idx].is_split {
            // Insert into micro-quadrant leaf intrusive list
            let old_head = self.hotspot_cells[h_idx].leaf_heads[leaf_idx];
            self.entity_next[id] = old_head;
            self.entity_prev[id] = QUADTREE_EMPTY;
            if old_head != QUADTREE_EMPTY {
                self.entity_prev[old_head as usize] = entity_id;
            }
            self.hotspot_cells[h_idx].leaf_heads[leaf_idx] = entity_id;
            self.hotspot_cells[h_idx].leaf_counts[leaf_idx] += 1;
        } else {
            // Insert into flat cell intrusive list
            let old_head = self.hotspot_cells[h_idx].flat_head;
            self.entity_next[id] = old_head;
            self.entity_prev[id] = QUADTREE_EMPTY;
            if old_head != QUADTREE_EMPTY {
                self.entity_prev[old_head as usize] = entity_id;
            }
            self.hotspot_cells[h_idx].flat_head = entity_id;
        }

        self.hotspot_cells[h_idx].entity_count += 1;

        // Check if dynamic split threshold reached
        if !self.hotspot_cells[h_idx].is_split
            && self.hotspot_cells[h_idx].entity_count >= HOTSPOT_SPLIT_THRESHOLD
        {
            self.split_cell(h_idx);
        }

        Ok(())
    }

    /// Removes an entity from the hotspot manager.
    pub fn remove(&mut self, entity_id: u32) -> Result<(), SpatialError> {
        let id = entity_id as usize;
        if id >= self.max_entities || !self.entity_active[id] {
            return Err(SpatialError::EntityNotFound(entity_id));
        }

        let h_id = self.entity_hotspot_id[id];
        if h_id == 0 {
            return Err(SpatialError::EntityNotFound(entity_id));
        }
        let h_idx = (h_id - 1) as usize;

        let next = self.entity_next[id];
        let prev = self.entity_prev[id];

        if self.hotspot_cells[h_idx].is_split {
            let leaf_idx = self.entity_leaf_idx[id] as usize;
            if prev != QUADTREE_EMPTY {
                self.entity_next[prev as usize] = next;
            } else {
                self.hotspot_cells[h_idx].leaf_heads[leaf_idx] = next;
            }

            if next != QUADTREE_EMPTY {
                self.entity_prev[next as usize] = prev;
            }

            self.hotspot_cells[h_idx].leaf_counts[leaf_idx] =
                self.hotspot_cells[h_idx].leaf_counts[leaf_idx].saturating_sub(1);
        } else {
            if prev != QUADTREE_EMPTY {
                self.entity_next[prev as usize] = next;
            } else {
                self.hotspot_cells[h_idx].flat_head = next;
            }

            if next != QUADTREE_EMPTY {
                self.entity_prev[next as usize] = prev;
            }
        }

        self.hotspot_cells[h_idx].entity_count =
            self.hotspot_cells[h_idx].entity_count.saturating_sub(1);
        self.entity_active[id] = false;
        self.entity_hotspot_id[id] = 0;
        self.entity_next[id] = QUADTREE_EMPTY;
        self.entity_prev[id] = QUADTREE_EMPTY;

        // Check if dynamic merge threshold reached (hysteresis)
        if self.hotspot_cells[h_idx].is_split
            && self.hotspot_cells[h_idx].entity_count <= HOTSPOT_MERGE_THRESHOLD
        {
            self.merge_cell(h_idx);
        }

        // Reclaim empty hotspot entry if 0 entities remain
        if self.hotspot_cells[h_idx].entity_count == 0 {
            self.reclaim_hotspot(h_idx);
        }

        Ok(())
    }

    /// Updates entity position, migrating between micro-quadrants or cells if necessary.
    pub fn update_position(
        &mut self,
        entity_id: u32,
        new_pos: Vec3Fix,
    ) -> Result<(), SpatialError> {
        let id = entity_id as usize;
        if id >= self.max_entities || !self.entity_active[id] {
            return Err(SpatialError::EntityNotFound(entity_id));
        }

        let old_cell = CellCoord::from_position(self.entity_positions[id]);
        let new_cell = CellCoord::from_position(new_pos);

        if old_cell != new_cell {
            // Cell migration: remove and insert
            self.remove(entity_id)?;
            self.insert(entity_id, new_pos)?;
        } else {
            // Same cell: update coordinates and micro-quadrant if split
            self.entity_positions[id] = new_pos;
            let h_idx = (self.entity_hotspot_id[id] - 1) as usize;

            if self.hotspot_cells[h_idx].is_split {
                let old_leaf = self.entity_leaf_idx[id] as usize;
                let new_leaf = self.hotspot_cells[h_idx].compute_leaf_index(new_pos);

                if old_leaf != new_leaf {
                    // Unlink from old leaf
                    let next = self.entity_next[id];
                    let prev = self.entity_prev[id];

                    if prev != QUADTREE_EMPTY {
                        self.entity_next[prev as usize] = next;
                    } else {
                        self.hotspot_cells[h_idx].leaf_heads[old_leaf] = next;
                    }
                    if next != QUADTREE_EMPTY {
                        self.entity_prev[next as usize] = prev;
                    }
                    self.hotspot_cells[h_idx].leaf_counts[old_leaf] =
                        self.hotspot_cells[h_idx].leaf_counts[old_leaf].saturating_sub(1);

                    // Prepend to new leaf
                    let old_head = self.hotspot_cells[h_idx].leaf_heads[new_leaf];
                    self.entity_next[id] = old_head;
                    self.entity_prev[id] = QUADTREE_EMPTY;
                    if old_head != QUADTREE_EMPTY {
                        self.entity_prev[old_head as usize] = entity_id;
                    }
                    self.hotspot_cells[h_idx].leaf_heads[new_leaf] = entity_id;
                    self.hotspot_cells[h_idx].leaf_counts[new_leaf] += 1;
                    self.entity_leaf_idx[id] = new_leaf as u8;
                }
            }
        }

        Ok(())
    }

    /// Dynamically splits a flat cell into 16 micro-quadrants.
    fn split_cell(&mut self, h_idx: usize) {
        self.hotspot_cells[h_idx].is_split = true;

        // Detach current flat list
        let mut curr = self.hotspot_cells[h_idx].flat_head;
        self.hotspot_cells[h_idx].flat_head = QUADTREE_EMPTY;

        // Reset leaf heads and counts
        for i in 0..MICRO_QUADRANTS_PER_CELL {
            self.hotspot_cells[h_idx].leaf_heads[i] = QUADTREE_EMPTY;
            self.hotspot_cells[h_idx].leaf_counts[i] = 0;
        }

        // Reassign every entity to its corresponding micro-quadrant
        while curr != QUADTREE_EMPTY {
            let id = curr as usize;
            let next = self.entity_next[id];

            let pos = self.entity_positions[id];
            let leaf_idx = self.hotspot_cells[h_idx].compute_leaf_index(pos);
            self.entity_leaf_idx[id] = leaf_idx as u8;

            let old_head = self.hotspot_cells[h_idx].leaf_heads[leaf_idx];
            self.entity_next[id] = old_head;
            self.entity_prev[id] = QUADTREE_EMPTY;
            if old_head != QUADTREE_EMPTY {
                self.entity_prev[old_head as usize] = curr;
            }
            self.hotspot_cells[h_idx].leaf_heads[leaf_idx] = curr;
            self.hotspot_cells[h_idx].leaf_counts[leaf_idx] += 1;

            curr = next;
        }
    }

    /// Dynamically collapses 16 micro-quadrants back into a flat root cell list (hysteresis).
    fn merge_cell(&mut self, h_idx: usize) {
        self.hotspot_cells[h_idx].is_split = false;

        let mut flat_head = QUADTREE_EMPTY;

        // Splice all 16 micro-quadrant linked lists into a single flat list
        for i in 0..MICRO_QUADRANTS_PER_CELL {
            let mut curr = self.hotspot_cells[h_idx].leaf_heads[i];
            self.hotspot_cells[h_idx].leaf_heads[i] = QUADTREE_EMPTY;
            self.hotspot_cells[h_idx].leaf_counts[i] = 0;

            while curr != QUADTREE_EMPTY {
                let id = curr as usize;
                let next = self.entity_next[id];

                self.entity_next[id] = flat_head;
                self.entity_prev[id] = QUADTREE_EMPTY;
                if flat_head != QUADTREE_EMPTY {
                    self.entity_prev[flat_head as usize] = curr;
                }
                flat_head = curr;

                curr = next;
            }
        }

        self.hotspot_cells[h_idx].flat_head = flat_head;
    }

    /// Reclaims an empty hotspot slot.
    fn reclaim_hotspot(&mut self, h_idx: usize) {
        if self.active_hotspots_count == 0 {
            return;
        }
        let last_idx = self.active_hotspots_count - 1;
        if h_idx != last_idx {
            // Swap with last active hotspot
            self.hotspot_cells.swap(h_idx, last_idx);

            // Update entity_hotspot_id for entities in the moved cell
            let moved_cell = &self.hotspot_cells[h_idx];
            if moved_cell.is_split {
                for i in 0..MICRO_QUADRANTS_PER_CELL {
                    let mut curr = moved_cell.leaf_heads[i];
                    while curr != QUADTREE_EMPTY {
                        self.entity_hotspot_id[curr as usize] = (h_idx + 1) as u16;
                        curr = self.entity_next[curr as usize];
                    }
                }
            } else {
                let mut curr = moved_cell.flat_head;
                while curr != QUADTREE_EMPTY {
                    self.entity_hotspot_id[curr as usize] = (h_idx + 1) as u16;
                    curr = self.entity_next[curr as usize];
                }
            }
        }

        self.active_hotspots_count -= 1;
    }

    /// Queries entities in a hotspot cell with hierarchical quadtree pruning.
    ///
    /// Evaluates 4 depth-1 quadrants first. If non-intersecting, all 4 leaves are pruned.
    /// For intersecting depth-1 quadrants, only intersecting micro-quadrant leaves are searched.
    pub fn query_cell_hierarchical(
        &self,
        h_idx: usize,
        center: Vec3Fix,
        radius_sq: Fixed64,
        output_buffer: &mut [u32],
        matched_count: &mut usize,
    ) {
        let cell = &self.hotspot_cells[h_idx];
        let cx = center.x;
        let cz = center.z;

        if !cell.is_split {
            // Flat traversal
            let mut curr = cell.flat_head;
            while curr != QUADTREE_EMPTY {
                let id = curr as usize;
                let dist_sq = self.entity_positions[id].distance_squared(center);
                if dist_sq <= radius_sq {
                    if *matched_count < output_buffer.len() {
                        output_buffer[*matched_count] = curr;
                    }
                    *matched_count += 1;
                }
                curr = self.entity_next[id];
            }
            return;
        }

        // Hierarchical Depth-1 Pruning (4 Quadrants: NW, NE, SW, SE)
        for q in 0..4 {
            if !cell.quadrant_aabbs[q].intersects_circle(cx, cz, radius_sq) {
                // Entire 32m quadrant does not intersect query circle: prune 4 micro-quadrant leaves
                continue;
            }

            // Quadrant intersects: evaluate its 4 micro-quadrant leaves
            let leaves = HotspotCell::quadrant_to_leaves(q);
            for &leaf_idx in &leaves {
                if cell.leaf_counts[leaf_idx] == 0 {
                    continue;
                }

                if !cell.leaf_aabbs[leaf_idx].intersects_circle(cx, cz, radius_sq) {
                    // Leaf does not intersect query circle: prune all entities in leaf
                    continue;
                }

                // Leaf intersects query circle: evaluate candidate entities
                let mut curr = cell.leaf_heads[leaf_idx];
                while curr != QUADTREE_EMPTY {
                    let id = curr as usize;
                    let dist_sq = self.entity_positions[id].distance_squared(center);
                    if dist_sq <= radius_sq {
                        if *matched_count < output_buffer.len() {
                            output_buffer[*matched_count] = curr;
                        }
                        *matched_count += 1;
                    }
                    curr = self.entity_next[id];
                }
            }
        }
    }

    /// Queries entities across all registered hotspot cells within radius_sq of center.
    pub fn query_radius(
        &self,
        center: Vec3Fix,
        radius_sq: Fixed64,
        output_buffer: &mut [u32],
    ) -> SpatialQueryResult {
        let mut matched_count = 0;

        for h_idx in 0..self.active_hotspots_count {
            self.query_cell_hierarchical(
                h_idx,
                center,
                radius_sq,
                output_buffer,
                &mut matched_count,
            );
        }

        SpatialQueryResult::new(matched_count.min(output_buffer.len()), matched_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quadtree_aabb_distance_and_circle_intersection() {
        let aabb = QuadtreeAabb::new(
            Fixed64::from_i32(10),
            Fixed64::from_i32(20),
            Fixed64::from_i32(10),
            Fixed64::from_i32(20),
        );

        // Point strictly inside
        let dist_inside =
            aabb.distance_squared_to_point(Fixed64::from_i32(15), Fixed64::from_i32(15));
        assert_eq!(dist_inside, Fixed64::ZERO);
        assert!(aabb.intersects_circle(
            Fixed64::from_i32(15),
            Fixed64::from_i32(15),
            Fixed64::from_i32(1)
        ));

        // Point directly to the left (5m away)
        let dist_left = aabb.distance_squared_to_point(Fixed64::from_i32(5), Fixed64::from_i32(15));
        assert_eq!(dist_left, Fixed64::from_i32(25)); // 5^2 = 25
        assert!(aabb.intersects_circle(
            Fixed64::from_i32(5),
            Fixed64::from_i32(15),
            Fixed64::from_i32(25)
        ));
        assert!(!aabb.intersects_circle(
            Fixed64::from_i32(5),
            Fixed64::from_i32(15),
            Fixed64::from_i32(24)
        ));

        // Corner point (diagonal: dx=3, dz=4 => dist_sq = 25)
        let dist_corner =
            aabb.distance_squared_to_point(Fixed64::from_i32(23), Fixed64::from_i32(24));
        assert_eq!(dist_corner, Fixed64::from_i32(25));
        assert!(aabb.intersects_circle(
            Fixed64::from_i32(23),
            Fixed64::from_i32(24),
            Fixed64::from_i32(25)
        ));
        assert!(!aabb.intersects_circle(
            Fixed64::from_i32(23),
            Fixed64::from_i32(24),
            Fixed64::from_i32(24)
        ));
    }

    #[test]
    fn test_hotspot_cell_quadrant_mapping_invariants() {
        let cell = CellCoord::new(1, 0, 1);
        let hotspot = HotspotCell::new(cell);

        // Cell spans x: 64..128, z: 64..128
        // Test 4 corner leaves
        let leaf_sw = hotspot.compute_leaf_index(Vec3Fix::from_f64(65.0, 0.0, 65.0));
        assert_eq!(leaf_sw, 0);
        assert_eq!(HotspotCell::leaf_to_quadrant(leaf_sw), 2); // SW quadrant

        let leaf_se = hotspot.compute_leaf_index(Vec3Fix::from_f64(125.0, 0.0, 65.0));
        assert_eq!(leaf_se, 3);
        assert_eq!(HotspotCell::leaf_to_quadrant(leaf_se), 3); // SE quadrant

        let leaf_nw = hotspot.compute_leaf_index(Vec3Fix::from_f64(65.0, 0.0, 125.0));
        assert_eq!(leaf_nw, 12);
        assert_eq!(HotspotCell::leaf_to_quadrant(leaf_nw), 0); // NW quadrant

        let leaf_ne = hotspot.compute_leaf_index(Vec3Fix::from_f64(125.0, 0.0, 125.0));
        assert_eq!(leaf_ne, 15);
        assert_eq!(HotspotCell::leaf_to_quadrant(leaf_ne), 1); // NE quadrant
    }

    #[test]
    fn test_dynamic_split_and_merge_hysteresis() {
        let mut manager = HotspotManager::new(400, 16);
        let cell = CellCoord::new(0, 0, 0);

        // Insert 249 entities (below split threshold)
        for i in 0..249 {
            let x = (i % 60) as f64 + 1.0;
            let z = ((i / 60) * 10) as f64 + 1.0;
            manager
                .insert(i as u32, Vec3Fix::from_f64(x, 0.0, z))
                .unwrap();
        }
        assert_eq!(manager.active_hotspot_count(), 1);
        assert!(!manager.is_split(cell));

        // 250th entity triggers split into 16 micro-quadrants
        manager
            .insert(249, Vec3Fix::from_f64(30.0, 0.0, 30.0))
            .unwrap();
        assert!(manager.is_split(cell));

        // Remove down to 101 (above merge threshold): remains split
        for i in 101..250 {
            manager.remove(i as u32).unwrap();
        }
        assert_eq!(manager.hotspot_cells[0].entity_count, 101);
        assert!(manager.is_split(cell));

        // Remove 100th: triggers merge back to flat cell
        manager.remove(100).unwrap();
        assert_eq!(manager.hotspot_cells[0].entity_count, 100);
        assert!(!manager.is_split(cell));
    }

    #[test]
    fn test_hierarchical_query_accuracy_parity() {
        let mut manager = HotspotManager::new(500, 16);

        // Cluster 300 entities in cell (0, 0, 0) to trigger split
        for i in 0..300 {
            let x = (i % 60) as f64 + 1.0;
            let z = ((i * 3) % 60) as f64 + 1.0;
            manager
                .insert(i as u32, Vec3Fix::from_f64(x, 0.0, z))
                .unwrap();
        }
        assert!(manager.is_split(CellCoord::new(0, 0, 0)));

        // Query 10m radius around (15.0, 0.0, 15.0)
        let center = Vec3Fix::from_f64(15.0, 0.0, 15.0);
        let radius_sq = Fixed64::from_i32(100); // 10m^2

        let mut quadtree_results = [0u32; 100];
        let query_res = manager.query_radius(center, radius_sq, &mut quadtree_results);

        // Ground-truth brute-force linear scan
        let mut expected_count = 0;
        for i in 0..300 {
            let dist_sq = manager.entity_positions[i].distance_squared(center);
            if dist_sq <= radius_sq {
                expected_count += 1;
            }
        }

        assert_eq!(query_res.total_matches, expected_count);
        assert!(query_res.total_matches > 0);
    }
}
