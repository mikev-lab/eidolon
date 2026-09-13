//! Cache-conscious spatial hash grid and cell coordinate partitioning.
//!
//! Provides constant-time O(1) entity insertion, removal, and cell-to-cell migration
//! using pre-allocated contiguous flat buffers and doubly-linked intrusive slot-map indexing.

use core::fmt;
use eidolon_core::fixed::{Fixed64, Vec3Fix};

/// Horizontal spatial cell size in meters (64 meters).
pub const CELL_HORIZONTAL_SIZE: i32 = 64;

/// Vertical spatial cell slice size in meters (32 meters).
pub const CELL_VERTICAL_SIZE: i32 = 32;

/// Sentinel index representing a terminal link in the intrusive slot map.
pub const TERMINAL_INDEX: u32 = u32::MAX;

/// Errors arising from spatial grid operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialError {
    /// Entity was not found in the grid.
    EntityNotFound(u32),
    /// Entity ID already exists in the grid.
    EntityAlreadyExists(u32),
    /// Grid capacity has been exhausted.
    CapacityExceeded,
    /// Specified entity ID exceeds configured capacity.
    InvalidEntityId(u32),
}

impl fmt::Display for SpatialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntityNotFound(id) => write!(f, "Entity {} not found in spatial grid", id),
            Self::EntityAlreadyExists(id) => {
                write!(f, "Entity {} already exists in spatial grid", id)
            }
            Self::CapacityExceeded => write!(f, "Spatial grid capacity exceeded"),
            Self::InvalidEntityId(id) => {
                write!(f, "Entity ID {} exceeds grid maximum capacity", id)
            }
        }
    }
}

/// Discrete 3D integer coordinate identifying a spatial cell bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CellCoord {
    /// Cell index on horizontal X axis.
    pub x: i32,
    /// Cell index on vertical Y axis.
    pub y: i32,
    /// Cell index on horizontal Z axis.
    pub z: i32,
}

impl CellCoord {
    /// Creates a new cell coordinate from discrete integer indices.
    #[inline]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Computes discrete cell coordinate from continuous fixed-point position.
    #[inline]
    pub fn from_position(pos: Vec3Fix) -> Self {
        let cx = pos.x.to_i32().div_euclid(CELL_HORIZONTAL_SIZE);
        let cy = pos.y.to_i32().div_euclid(CELL_VERTICAL_SIZE);
        let cz = pos.z.to_i32().div_euclid(CELL_HORIZONTAL_SIZE);
        Self {
            x: cx,
            y: cy,
            z: cz,
        }
    }

    /// Computes a 64-bit spatial coordinate key.
    ///
    /// Packs 24-bit signed X, 24-bit signed Z, and 16-bit signed Y into a 64-bit integer,
    /// providing an injective coordinate space exceeding 500,000 kilometers.
    #[inline]
    pub const fn spatial_key(self) -> u64 {
        let x_bits = (self.x as u64) & 0x00FF_FFFF;
        let z_bits = (self.z as u64) & 0x00FF_FFFF;
        let y_bits = (self.y as u64) & 0x0000_FFFF;
        (x_bits << 40) | (z_bits << 16) | y_bits
    }
}

/// High-density cache-aligned spatial hash grid.
///
/// Backed by pre-allocated contiguous flat arrays with intrusive doubly-linked slot indexing,
/// ensuring zero dynamic heap allocations during simulation ticks.
#[derive(Debug)]
pub struct SpatialHashGrid {
    max_entities: usize,
    num_buckets: usize,
    bucket_mask: usize,
    active_count: usize,

    // Struct-of-Arrays (SoA) contiguous flat storage
    positions: Vec<Vec3Fix>,
    cell_coords: Vec<CellCoord>,
    entity_keys: Vec<u64>,
    active_mask: Vec<bool>,
    next_in_cell: Vec<u32>,
    prev_in_cell: Vec<u32>,

    // Hash table bucket heads
    bucket_heads: Vec<u32>,
}

impl SpatialHashGrid {
    /// Constructs a spatial hash grid with default bucket capacity scaling.
    pub fn new(max_entities: usize) -> Self {
        Self::with_capacity(max_entities, max_entities.max(64) * 2)
    }

    /// Constructs a spatial hash grid with pre-allocated capacities.
    ///
    /// The number of hash buckets is rounded up to the nearest power of two for branchless masking.
    pub fn with_capacity(max_entities: usize, requested_buckets: usize) -> Self {
        let num_buckets = requested_buckets.next_power_of_two().max(64);
        let bucket_mask = num_buckets - 1;

        Self {
            max_entities,
            num_buckets,
            bucket_mask,
            active_count: 0,
            positions: vec![Vec3Fix::ZERO; max_entities],
            cell_coords: vec![CellCoord::default(); max_entities],
            entity_keys: vec![0; max_entities],
            active_mask: vec![false; max_entities],
            next_in_cell: vec![TERMINAL_INDEX; max_entities],
            prev_in_cell: vec![TERMINAL_INDEX; max_entities],
            bucket_heads: vec![TERMINAL_INDEX; num_buckets],
        }
    }

    /// Returns the maximum entity capacity of the grid.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.max_entities
    }

    /// Returns the number of active entities in the grid.
    #[inline]
    pub fn active_count(&self) -> usize {
        self.active_count
    }

    /// Returns the number of allocated hash buckets in the grid.
    #[inline]
    pub fn num_buckets(&self) -> usize {
        self.num_buckets
    }

    /// Returns true if the specified entity ID is active in the grid.
    #[inline]
    pub fn contains(&self, entity_id: u32) -> bool {
        let id = entity_id as usize;
        if id < self.max_entities {
            self.active_mask[id]
        } else {
            false
        }
    }

    /// Returns the current position of an entity, or None if not found.
    #[inline]
    pub fn get_position(&self, entity_id: u32) -> Option<Vec3Fix> {
        let id = entity_id as usize;
        if id < self.max_entities && self.active_mask[id] {
            Some(self.positions[id])
        } else {
            None
        }
    }

    /// Returns the current cell coordinate of an entity, or None if not found.
    #[inline]
    pub fn get_cell(&self, entity_id: u32) -> Option<CellCoord> {
        let id = entity_id as usize;
        if id < self.max_entities && self.active_mask[id] {
            Some(self.cell_coords[id])
        } else {
            None
        }
    }

    /// Inserts an entity at the specified continuous position.
    ///
    /// Executes in constant time O(1) without heap allocation.
    pub fn insert(&mut self, entity_id: u32, position: Vec3Fix) -> Result<(), SpatialError> {
        let id = entity_id as usize;
        if id >= self.max_entities {
            return Err(SpatialError::InvalidEntityId(entity_id));
        }
        if self.active_mask[id] {
            return Err(SpatialError::EntityAlreadyExists(entity_id));
        }

        let cell = CellCoord::from_position(position);
        let key = cell.spatial_key();
        let bucket_idx = (key as usize) & self.bucket_mask;

        self.positions[id] = position;
        self.cell_coords[id] = cell;
        self.entity_keys[id] = key;
        self.active_mask[id] = true;

        // Prepend to bucket intrusive list
        let old_head = self.bucket_heads[bucket_idx];
        self.next_in_cell[id] = old_head;
        self.prev_in_cell[id] = TERMINAL_INDEX;

        if old_head != TERMINAL_INDEX {
            self.prev_in_cell[old_head as usize] = entity_id;
        }
        self.bucket_heads[bucket_idx] = entity_id;
        self.active_count += 1;

        Ok(())
    }

    /// Updates the position of an active entity.
    ///
    /// If the entity crosses a spatial cell boundary, its bucket linkage is updated
    /// in constant time O(1). Returns Ok(true) if a cell boundary was crossed.
    pub fn update_position(
        &mut self,
        entity_id: u32,
        new_position: Vec3Fix,
    ) -> Result<bool, SpatialError> {
        let id = entity_id as usize;
        if id >= self.max_entities || !self.active_mask[id] {
            return Err(SpatialError::EntityNotFound(entity_id));
        }

        self.positions[id] = new_position;
        let new_cell = CellCoord::from_position(new_position);

        // Fast path: entity remained within the same cell bucket
        if new_cell == self.cell_coords[id] {
            return Ok(false);
        }

        // Seam crossed: detach from old bucket
        let old_key = self.entity_keys[id];
        let old_bucket = (old_key as usize) & self.bucket_mask;
        self.detach_from_bucket(entity_id, old_bucket);

        // Attach to new bucket
        let new_key = new_cell.spatial_key();
        let new_bucket = (new_key as usize) & self.bucket_mask;

        self.cell_coords[id] = new_cell;
        self.entity_keys[id] = new_key;
        self.attach_to_bucket(entity_id, new_bucket);

        Ok(true)
    }

    /// Removes an entity from the grid in constant time O(1).
    pub fn remove(&mut self, entity_id: u32) -> Result<(), SpatialError> {
        let id = entity_id as usize;
        if id >= self.max_entities || !self.active_mask[id] {
            return Err(SpatialError::EntityNotFound(entity_id));
        }

        let key = self.entity_keys[id];
        let bucket = (key as usize) & self.bucket_mask;

        self.detach_from_bucket(entity_id, bucket);
        self.active_mask[id] = false;
        self.next_in_cell[id] = TERMINAL_INDEX;
        self.prev_in_cell[id] = TERMINAL_INDEX;
        self.active_count -= 1;

        Ok(())
    }

    /// Queries all entities within a squared radius of a continuous point.
    ///
    /// Scans the bounded 3x3 neighborhood of cells centered on the point.
    /// Matches are written directly into `output_buffer` without heap allocation.
    /// Returns the total number of matched entities.
    pub fn query_radius_squared(
        &self,
        center: Vec3Fix,
        radius_sq: Fixed64,
        output_buffer: &mut [u32],
    ) -> usize {
        let center_cell = CellCoord::from_position(center);
        let mut matched_count = 0;

        // 3x3 cell neighborhood iteration around observer
        for dx in -1..=1 {
            for dz in -1..=1 {
                for dy in -1..=1 {
                    let neighbor_cell =
                        CellCoord::new(center_cell.x + dx, center_cell.y + dy, center_cell.z + dz);
                    let key = neighbor_cell.spatial_key();
                    let bucket = (key as usize) & self.bucket_mask;

                    let mut curr = self.bucket_heads[bucket];
                    while curr != TERMINAL_INDEX {
                        let curr_idx = curr as usize;
                        if self.entity_keys[curr_idx] == key && self.active_mask[curr_idx] {
                            let dist_sq = self.positions[curr_idx].distance_squared(center);
                            if dist_sq <= radius_sq {
                                if matched_count < output_buffer.len() {
                                    output_buffer[matched_count] = curr;
                                }
                                matched_count += 1;
                            }
                        }
                        curr = self.next_in_cell[curr_idx];
                    }
                }
            }
        }

        matched_count
    }

    /// Queries active entities within a squared radius of the center point using 4-wide SIMD batching.
    ///
    /// Batches candidate entity position checks into 4-wide parallel distance evaluations,
    /// enabling SIMD auto-vectorization across high-density spatial cells.
    /// Matches are written directly into `output_buffer` without heap allocation.
    pub fn query_radius_squared_batched(
        &self,
        center: Vec3Fix,
        radius_sq: Fixed64,
        output_buffer: &mut [u32],
    ) -> usize {
        let center_cell = CellCoord::from_position(center);
        let mut matched_count = 0;

        let mut batch_candidates = [0u32; 4];
        let mut batch_positions = [Vec3Fix::ZERO; 4];
        let mut batch_len = 0;

        // 3x3 cell neighborhood iteration around observer
        for dx in -1..=1 {
            for dz in -1..=1 {
                for dy in -1..=1 {
                    let neighbor_cell =
                        CellCoord::new(center_cell.x + dx, center_cell.y + dy, center_cell.z + dz);
                    let key = neighbor_cell.spatial_key();
                    let bucket = (key as usize) & self.bucket_mask;

                    let mut curr = self.bucket_heads[bucket];
                    while curr != TERMINAL_INDEX {
                        let curr_idx = curr as usize;
                        if self.entity_keys[curr_idx] == key && self.active_mask[curr_idx] {
                            batch_candidates[batch_len] = curr;
                            batch_positions[batch_len] = self.positions[curr_idx];
                            batch_len += 1;

                            if batch_len == 4 {
                                let dists =
                                    Vec3Fix::batch_distance_squared_4x(batch_positions, center);
                                for i in 0..4 {
                                    if dists[i] <= radius_sq {
                                        if matched_count < output_buffer.len() {
                                            output_buffer[matched_count] = batch_candidates[i];
                                        }
                                        matched_count += 1;
                                    }
                                }
                                batch_len = 0;
                            }
                        }
                        curr = self.next_in_cell[curr_idx];
                    }
                }
            }
        }

        // Process remaining tail candidates
        if batch_len > 0 {
            batch_positions[batch_len..4].fill(center);
            let dists = Vec3Fix::batch_distance_squared_4x(batch_positions, center);
            for i in 0..batch_len {
                if dists[i] <= radius_sq {
                    if matched_count < output_buffer.len() {
                        output_buffer[matched_count] = batch_candidates[i];
                    }
                    matched_count += 1;
                }
            }
        }

        matched_count
    }

    /// Counts active entities residing in a specific cell.
    pub fn cell_entity_count(&self, cell: CellCoord) -> usize {
        let key = cell.spatial_key();
        let bucket = (key as usize) & self.bucket_mask;
        let mut count = 0;

        let mut curr = self.bucket_heads[bucket];
        while curr != TERMINAL_INDEX {
            let curr_idx = curr as usize;
            if self.entity_keys[curr_idx] == key && self.active_mask[curr_idx] {
                count += 1;
            }
            curr = self.next_in_cell[curr_idx];
        }

        count
    }

    #[inline]
    fn detach_from_bucket(&mut self, entity_id: u32, bucket_idx: usize) {
        let id = entity_id as usize;
        let prev = self.prev_in_cell[id];
        let next = self.next_in_cell[id];

        if prev == TERMINAL_INDEX {
            // Entity was head of bucket
            self.bucket_heads[bucket_idx] = next;
        } else {
            self.next_in_cell[prev as usize] = next;
        }

        if next != TERMINAL_INDEX {
            self.prev_in_cell[next as usize] = prev;
        }

        self.next_in_cell[id] = TERMINAL_INDEX;
        self.prev_in_cell[id] = TERMINAL_INDEX;
    }

    #[inline]
    fn attach_to_bucket(&mut self, entity_id: u32, bucket_idx: usize) {
        let id = entity_id as usize;
        let old_head = self.bucket_heads[bucket_idx];

        self.next_in_cell[id] = old_head;
        self.prev_in_cell[id] = TERMINAL_INDEX;

        if old_head != TERMINAL_INDEX {
            self.prev_in_cell[old_head as usize] = entity_id;
        }
        self.bucket_heads[bucket_idx] = entity_id;
    }
}
