//! High-density cache-aligned spatial hash grid and tiered Area of Interest (AoI) frequency manager.
//!
//! `eidolon-spatial` partitions entities into contiguous spatial buckets, enabling constant-time
//! insertion, removal, and distance-tiered replication scheduling.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod aoi;
pub mod grid;
pub mod tier;

pub use aoi::{
    AoIScheduler, LoadSheddingLevel, ObserverInterestSet, VisibilityEvent, MAX_AOI_RADIUS_METERS,
    MAX_AOI_RADIUS_SQ,
};
pub use grid::{
    CellCoord, SpatialError, SpatialHashGrid, CELL_HORIZONTAL_SIZE, CELL_VERTICAL_SIZE,
};
pub use tier::{
    FrequencyTier, IMMEDIATE_DEMOTION_DIST_SQ, IMMEDIATE_PROMOTION_DIST_SQ, MID_DEMOTION_DIST_SQ,
    MID_PROMOTION_DIST_SQ,
};

#[cfg(test)]
mod tests {
    use super::*;
    use eidolon_core::fixed::{Fixed64, Vec3Fix};

    #[test]
    fn test_cell_coord_calculation() {
        let pos = Vec3Fix::from_f64(130.0, 40.0, -10.0);
        let cell = CellCoord::from_position(pos);

        // 130 / 64 = 2 (128..192)
        assert_eq!(cell.x, 2);
        // 40 / 32 = 1 (32..64)
        assert_eq!(cell.y, 1);
        // -10.div_euclid(64) = -1 (-64..0)
        assert_eq!(cell.z, -1);
    }

    #[test]
    fn test_grid_insertion_and_query() {
        let mut grid = SpatialHashGrid::with_capacity(100, 64);
        assert_eq!(grid.active_count(), 0);

        let e1_pos = Vec3Fix::from_f64(10.0, 5.0, 10.0);
        let e2_pos = Vec3Fix::from_f64(15.0, 5.0, 10.0); // 5m from e1
        let e3_pos = Vec3Fix::from_f64(100.0, 5.0, 10.0); // 90m from e1 (different cell)

        assert!(grid.insert(1, e1_pos).is_ok());
        assert!(grid.insert(2, e2_pos).is_ok());
        assert!(grid.insert(3, e3_pos).is_ok());
        assert_eq!(grid.active_count(), 3);

        // Query within 10m radius of e1 (radius_sq = 100)
        let mut query_results = [0u32; 16];
        let count = grid.query_radius_squared(e1_pos, Fixed64::from_i32(100), &mut query_results);

        assert_eq!(count, 2);
        let matched: Vec<u32> = query_results[..count].to_vec();
        assert!(matched.contains(&1));
        assert!(matched.contains(&2));
        assert!(!matched.contains(&3));
    }

    #[test]
    fn test_grid_movement_and_removal() {
        let mut grid = SpatialHashGrid::with_capacity(50, 64);
        let initial_pos = Vec3Fix::from_f64(10.0, 0.0, 10.0);
        assert!(grid.insert(5, initial_pos).is_ok());

        // Intra-cell movement: does not cross cell seam
        let inside_cell = Vec3Fix::from_f64(20.0, 0.0, 20.0);
        assert_eq!(grid.update_position(5, inside_cell), Ok(false));

        // Cross-cell movement: moves to cell (2, 0, 0)
        let across_seam = Vec3Fix::from_f64(150.0, 0.0, 10.0);
        assert_eq!(grid.update_position(5, across_seam), Ok(true));

        // Verify entity position updated
        assert_eq!(grid.get_position(5), Some(across_seam));

        // Removal
        assert!(grid.remove(5).is_ok());
        assert_eq!(grid.active_count(), 0);
        assert_eq!(grid.get_position(5), None);
    }

    #[test]
    fn test_frequency_tier_hysteresis() {
        let initial = FrequencyTier::Immediate;

        // At 10m (100 m^2): within deadband [81, 121], remains Immediate
        let dist_10m = Fixed64::from_i32(100);
        assert_eq!(
            initial.update_with_hysteresis(dist_10m),
            FrequencyTier::Immediate
        );

        // Moves to 12m (144 m^2): exceeds 121 m^2 demotion threshold, demotes to Mid
        let dist_12m = Fixed64::from_i32(144);
        let mid_tier = initial.update_with_hysteresis(dist_12m);
        assert_eq!(mid_tier, FrequencyTier::Mid);

        // Moves back to 10m (100 m^2): within deadband [81, 121], remains Mid
        assert_eq!(
            mid_tier.update_with_hysteresis(dist_10m),
            FrequencyTier::Mid
        );

        // Moves to 8m (64 m^2): drops below 81 m^2 promotion threshold, promotes to Immediate
        let dist_8m = Fixed64::from_i32(64);
        assert_eq!(
            mid_tier.update_with_hysteresis(dist_8m),
            FrequencyTier::Immediate
        );
    }

    #[test]
    fn test_load_shedding_dispatch() {
        let mut scheduler = AoIScheduler::new();

        // Normal operation: Immediate 10 Hz (every 2 ticks), Mid 2 Hz (every 10 ticks)
        assert!(scheduler.should_replicate(FrequencyTier::Immediate, 2, false));
        assert!(!scheduler.should_replicate(FrequencyTier::Immediate, 3, false));
        assert!(scheduler.should_replicate(FrequencyTier::Mid, 10, false));
        assert!(!scheduler.should_replicate(FrequencyTier::Mid, 2, false));

        // Level 1 load shedding: Mid throttled to 1 Hz (every 20 ticks)
        scheduler.set_load_shedding(LoadSheddingLevel::Level1);
        assert!(scheduler.should_replicate(FrequencyTier::Immediate, 2, false));
        assert!(!scheduler.should_replicate(FrequencyTier::Mid, 10, false));
        assert!(scheduler.should_replicate(FrequencyTier::Mid, 20, false));

        // Level 2 load shedding: Mid dropped entirely
        scheduler.set_load_shedding(LoadSheddingLevel::Level2);
        assert!(scheduler.should_replicate(FrequencyTier::Immediate, 2, false));
        assert!(!scheduler.should_replicate(FrequencyTier::Mid, 20, false));
    }
}
