//! High-density cache-aligned spatial hash grid and tiered Area of Interest (AoI) frequency manager.
//!
//! `eidolon-spatial` partitions entities into contiguous spatial buckets, enabling constant-time
//! insertion, removal, and distance-tiered replication scheduling.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod aoi;
pub mod bvh;
pub mod grid;
pub mod hlod;
pub mod tier;

pub use aoi::{
    calculate_relevance_score, AoIScheduler, EntityRelation, LoadSheddingLevel,
    ObserverInterestSet, RelevanceWeights, VisibilityEvent, MAX_AOI_5TIER_RADIUS_SQ,
    MAX_AOI_RADIUS_METERS, MAX_AOI_RADIUS_SQ,
};
pub use bvh::{BvhLeafPiece, CompoundStructure, RayHit};
pub use grid::{
    CellCoord, SpatialError, SpatialHashGrid, SpatialQueryResult, CELL_HORIZONTAL_SIZE,
    CELL_VERTICAL_SIZE,
};
pub use hlod::{HlodGrid, MacroTileCoord, TerrainTile, MACRO_TILE_EDGE_METERS};
pub use tier::{
    FrequencyTier, TierModel, IMMEDIATE_DEMOTION_DIST_SQ, IMMEDIATE_PROMOTION_DIST_SQ,
    MID_DEMOTION_DIST_SQ, MID_PROMOTION_DIST_SQ, TIER5_HORIZON_DEMOTION_DIST_SQ,
    TIER5_HORIZON_PROMOTION_DIST_SQ, TIER5_IMMEDIATE_DEMOTION_DIST_SQ,
    TIER5_IMMEDIATE_PROMOTION_DIST_SQ, TIER5_MIDFIELD_DEMOTION_DIST_SQ,
    TIER5_MIDFIELD_PROMOTION_DIST_SQ, TIER5_TACTICAL_DEMOTION_DIST_SQ,
    TIER5_TACTICAL_PROMOTION_DIST_SQ,
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
        let res = grid.query_radius_squared(e1_pos, Fixed64::from_i32(100), &mut query_results);

        assert_eq!(res.written, 2);
        assert_eq!(res.total_matches, 2);
        assert!(!res.is_truncated());
        let matched: Vec<u32> = query_results[..res.written].to_vec();
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

    #[test]
    fn test_query_radius_squared_batched_parity() {
        let mut grid = SpatialHashGrid::with_capacity(100, 64);
        let center = Vec3Fix::from_f64(30.0, 10.0, 30.0);

        // Insert 15 entities at various positions
        for i in 1..=15 {
            let offset = (i as f64) * 3.0;
            let pos = Vec3Fix::from_f64(30.0 + offset, 10.0, 30.0);
            assert!(grid.insert(i, pos).is_ok());
        }

        let radius_sq = Fixed64::from_i32(400); // 20m radius (20^2 = 400)
        let mut scalar_results = [0u32; 32];
        let mut batched_results = [0u32; 32];

        let scalar_res = grid.query_radius_squared(center, radius_sq, &mut scalar_results);
        let batched_res =
            grid.query_radius_squared_batched(center, radius_sq, &mut batched_results);

        assert_eq!(
            scalar_res.written, batched_res.written,
            "Batched query count must match scalar query count"
        );
        assert_eq!(scalar_res.total_matches, batched_res.total_matches);
        let mut scalar_matched = scalar_results[..scalar_res.written].to_vec();
        let mut batched_matched = batched_results[..batched_res.written].to_vec();
        scalar_matched.sort_unstable();
        batched_matched.sort_unstable();
        assert_eq!(
            scalar_matched, batched_matched,
            "Batched query matches must be identical to scalar query"
        );
    }

    #[test]
    fn test_vertical_query_extent_50m() {
        let mut grid = SpatialHashGrid::with_capacity(50, 64);

        // Observer at (0, 33, 0).
        // Y=33 is in vertical cell y = 1 (range [32, 64)).
        let observer_pos = Vec3Fix::from_f64(0.0, 33.0, 0.0);
        assert_eq!(CellCoord::from_position(observer_pos).y, 1);

        // Entity 1 at (0, -10, 0).
        // Y=-10 is in vertical cell y = -1 (range [-32, 0)).
        // Vertical difference in cells: 1 - (-1) = 2 cells (dy = -2).
        // Euclidean distance: 33 - (-10) = 43 meters <= 50m AoI radius!
        let entity_pos = Vec3Fix::from_f64(0.0, -10.0, 0.0);
        assert_eq!(CellCoord::from_position(entity_pos).y, -1);
        assert!(grid.insert(42, entity_pos).is_ok());

        let mut output = [0u32; 16];
        let res = grid.query_radius_squared(observer_pos, MAX_AOI_RADIUS_SQ, &mut output);

        assert_eq!(res.written, 1);
        assert_eq!(res.total_matches, 1);
        assert_eq!(output[0], 42);

        // Also verify batched query discovers the entity across dy = -2
        let mut batched_output = [0u32; 16];
        let batched_res =
            grid.query_radius_squared_batched(observer_pos, MAX_AOI_RADIUS_SQ, &mut batched_output);
        assert_eq!(batched_res.written, 1);
        assert_eq!(batched_output[0], 42);
    }

    #[test]
    fn test_spatial_query_result_truncation() {
        let mut grid = SpatialHashGrid::with_capacity(50, 64);
        let center = Vec3Fix::from_f64(0.0, 0.0, 0.0);

        for id in 1..=5 {
            assert!(grid
                .insert(id, Vec3Fix::from_f64(id as f64, 0.0, 0.0))
                .is_ok());
        }

        // Buffer only holds 2 entities, but 5 match
        let mut small_buffer = [0u32; 2];
        let res = grid.query_radius_squared(center, Fixed64::from_i32(100), &mut small_buffer);

        assert_eq!(res.written, 2);
        assert_eq!(res.total_matches, 5);
        assert!(res.is_truncated());
    }

    #[test]
    fn test_large_radius_query_multi_cell() {
        let mut grid = SpatialHashGrid::with_capacity(50, 128);
        let center = Vec3Fix::ZERO;

        // Place entity at 120m along X axis (which is cell x = 1 or 2 depending on origin)
        // 120m / 64m = cell x = 1 (range [64, 128)).
        // Place another entity at 150m along X axis: 150m / 64m = cell x = 2 (range [128, 192)).
        assert!(grid.insert(1, Vec3Fix::from_f64(120.0, 0.0, 0.0)).is_ok());
        assert!(grid.insert(2, Vec3Fix::from_f64(150.0, 0.0, 0.0)).is_ok());

        // Query with radius 130m (radius_sq = 16900): should find entity 1 (120m <= 130m) but not entity 2 (150m > 130m)
        let mut buffer = [0u32; 16];
        let res = grid.query_radius_squared(center, Fixed64::from_i32(130 * 130), &mut buffer);
        assert_eq!(res.written, 1);
        assert_eq!(buffer[0], 1);

        // Query with radius 160m (radius_sq = 25600): should find both entity 1 and entity 2
        let res_large =
            grid.query_radius_squared(center, Fixed64::from_i32(160 * 160), &mut buffer);
        assert_eq!(res_large.written, 2);
    }

    #[test]
    fn test_continuous_5tier_classification_and_payload_sizes() {
        assert_eq!(FrequencyTier::Immediate.payload_size_bytes(), 7);
        assert_eq!(FrequencyTier::Tactical.payload_size_bytes(), 7);
        assert_eq!(FrequencyTier::Mid.payload_size_bytes(), 5);
        assert_eq!(FrequencyTier::Midfield.payload_size_bytes(), 5);
        assert_eq!(FrequencyTier::Horizon.payload_size_bytes(), 4);
        assert_eq!(FrequencyTier::Macro.payload_size_bytes(), 2);

        // Initial classification
        assert_eq!(
            FrequencyTier::classify_5tier(Fixed64::from_i32(100)),
            FrequencyTier::Immediate
        );
        assert_eq!(
            FrequencyTier::classify_5tier(Fixed64::from_i32(400)),
            FrequencyTier::Tactical
        );
        assert_eq!(
            FrequencyTier::classify_5tier(Fixed64::from_i32(2500)),
            FrequencyTier::Midfield
        );
        assert_eq!(
            FrequencyTier::classify_5tier(Fixed64::from_i32(40000)),
            FrequencyTier::Horizon
        );
        assert_eq!(
            FrequencyTier::classify_5tier(Fixed64::from_i32(100000)),
            FrequencyTier::Macro
        );

        // 5-tier hysteresis: loitering in deadbands
        let immediate = FrequencyTier::Immediate;
        // 15m squared = 225 (within [196, 256] deadband)
        assert_eq!(
            immediate.update_5tier_with_hysteresis(Fixed64::from_i32(225)),
            FrequencyTier::Immediate
        );
        // Exceeds 256: demotes to Tactical
        let tactical = immediate.update_5tier_with_hysteresis(Fixed64::from_i32(300));
        assert_eq!(tactical, FrequencyTier::Tactical);
        // Drops below 196: promotes to Immediate
        assert_eq!(
            tactical.update_5tier_with_hysteresis(Fixed64::from_i32(180)),
            FrequencyTier::Immediate
        );
    }

    #[test]
    fn test_priority_relevance_ranking() {
        let mut grid = SpatialHashGrid::with_capacity(20, 64);
        let obs_pos = Vec3Fix::ZERO;

        // Entity 1: at 10m, neutral
        assert!(grid.insert(1, Vec3Fix::from_f64(10.0, 0.0, 0.0)).is_ok());
        // Entity 2: at 20m, targeted focus
        assert!(grid.insert(2, Vec3Fix::from_f64(20.0, 0.0, 0.0)).is_ok());
        // Entity 3: at 30m, party member
        assert!(grid.insert(3, Vec3Fix::from_f64(30.0, 0.0, 0.0)).is_ok());

        let mut interest_set = ObserverInterestSet::with_capacity(2);
        interest_set.set_tier_model(TierModel::Continuous5Tier);

        let candidates = [1, 2, 3];
        let mut events = [VisibilityEvent::Exit { entity_id: 0 }; 8];

        let count = interest_set.update_visibility_ranked(
            &grid,
            obs_pos,
            &candidates,
            |id| match id {
                2 => EntityRelation {
                    is_target: true,
                    ..Default::default()
                },
                3 => EntityRelation {
                    is_party: true,
                    ..Default::default()
                },
                _ => EntityRelation::default(),
            },
            &mut events,
        );

        assert_eq!(count, 2);
        let visible = interest_set.visible_entities();
        // Since interest_set capacity is 2, the targeted entity (2) and party member (3) must be retained
        // while the closer neutral entity (1) is deprioritized
        assert!(visible.contains(&2));
        assert!(visible.contains(&3));
        assert!(!visible.contains(&1));
    }

    #[test]
    fn test_query_radius_morton_parity() {
        let mut grid = SpatialHashGrid::with_capacity(100, 64);
        let center = Vec3Fix::from_f64(30.0, 10.0, 30.0);

        for id in 1..=30 {
            let offset = (id as f64) * 2.0;
            let pos = Vec3Fix::from_f64(30.0 + offset, 10.0, 30.0);
            assert!(grid.insert(id, pos).is_ok());
            assert!(grid.get_morton_code(id).is_some());
        }

        let radius_sq = Fixed64::from_i32(400); // 20m radius

        let mut regular_buf = [0u32; 32];
        let mut morton_buf = [0u32; 32];

        let regular_res = grid.query_radius_squared(center, radius_sq, &mut regular_buf);
        let morton_res = grid.query_radius_morton(center, radius_sq, &mut morton_buf);

        assert_eq!(regular_res.written, morton_res.written);
        assert_eq!(regular_res.total_matches, morton_res.total_matches);

        let mut reg_sorted = regular_buf[..regular_res.written].to_vec();
        let mut mor_sorted = morton_buf[..morton_res.written].to_vec();
        reg_sorted.sort_unstable();
        mor_sorted.sort_unstable();
        assert_eq!(reg_sorted, mor_sorted);
    }

    #[test]
    fn test_query_radius_8wide_simd_parity() {
        let mut grid = SpatialHashGrid::with_capacity(120, 64);
        let center = Vec3Fix::from_f64(50.0, 15.0, 50.0);

        for id in 1..=60 {
            let angle = (id as f64) * 0.3;
            let dist = (id as f64) * 1.2;
            let pos = Vec3Fix::from_f64(
                50.0 + dist * angle.cos(),
                15.0 + (id as f64 % 5.0),
                50.0 + dist * angle.sin(),
            );
            assert!(grid.insert(id, pos).is_ok());
        }

        let radius_sq = Fixed64::from_i32(900); // 30m radius

        let mut scalar_buf = [0u32; 64];
        let mut batched4x_buf = [0u32; 64];
        let mut batched8x_buf = [0u32; 64];

        let scalar_res = grid.query_radius_squared(center, radius_sq, &mut scalar_buf);
        let batched4x_res =
            grid.query_radius_squared_batched(center, radius_sq, &mut batched4x_buf);
        let batched8x_res =
            grid.query_radius_squared_batched_8x(center, radius_sq, &mut batched8x_buf);

        assert_eq!(scalar_res.written, batched4x_res.written);
        assert_eq!(scalar_res.written, batched8x_res.written);
        assert_eq!(scalar_res.total_matches, batched8x_res.total_matches);

        let mut scalar_sorted = scalar_buf[..scalar_res.written].to_vec();
        let mut batched8x_sorted = batched8x_buf[..batched8x_res.written].to_vec();
        scalar_sorted.sort_unstable();
        batched8x_sorted.sort_unstable();
        assert_eq!(scalar_sorted, batched8x_sorted);
    }

    #[test]
    fn test_query_radius_16wide_simd_parity() {
        let mut grid = SpatialHashGrid::with_capacity(120, 64);
        let center = Vec3Fix::from_f64(50.0, 15.0, 50.0);

        for id in 1..=60 {
            let angle = (id as f64) * 0.3;
            let dist = (id as f64) * 1.2;
            let pos = Vec3Fix::from_f64(
                50.0 + dist * angle.cos(),
                15.0 + (id as f64 % 5.0),
                50.0 + dist * angle.sin(),
            );
            assert!(grid.insert(id, pos).is_ok());
        }

        let radius_sq = Fixed64::from_i32(900); // 30m radius

        let mut scalar_buf = [0u32; 64];
        let mut batched16x_buf = [0u32; 64];

        let scalar_res = grid.query_radius_squared(center, radius_sq, &mut scalar_buf);
        let batched16x_res =
            grid.query_radius_squared_batched_16x(center, radius_sq, &mut batched16x_buf);

        assert_eq!(scalar_res.written, batched16x_res.written);
        assert_eq!(scalar_res.total_matches, batched16x_res.total_matches);

        let mut scalar_sorted = scalar_buf[..scalar_res.written].to_vec();
        let mut batched16x_sorted = batched16x_buf[..batched16x_res.written].to_vec();
        scalar_sorted.sort_unstable();
        batched16x_sorted.sort_unstable();
        assert_eq!(
            scalar_sorted, batched16x_sorted,
            "16-wide SIMD query results must match scalar query"
        );
    }

    #[test]
    fn test_query_vision_cone_16wide_culling() {
        let mut grid = SpatialHashGrid::with_capacity(30, 64);
        let observer = Vec3Fix::ZERO;
        let forward = Vec3Fix::new(Fixed64::ZERO, Fixed64::ZERO, Fixed64::ONE);
        let cos_half_sq = Fixed64::from_f64(0.5); // 90 degree FoV (half-angle 45 deg)
        let max_range_sq = Fixed64::from_i32(2500); // 50m max viewing distance
        let personal_space_sq = Fixed64::from_i32(1); // 1m personal space

        // Entity 1: Directly ahead at 20m (inside cone)
        assert!(grid.insert(1, Vec3Fix::from_f64(0.0, 0.0, 20.0)).is_ok());
        // Entity 2: Forward-right at (10, 0, 20), ~26.5 deg angle (inside cone)
        assert!(grid.insert(2, Vec3Fix::from_f64(10.0, 0.0, 20.0)).is_ok());
        // Entity 3: Directly behind at -10m (outside cone, dot < 0)
        assert!(grid.insert(3, Vec3Fix::from_f64(0.0, 0.0, -10.0)).is_ok());
        // Entity 4: Wide flank at (40, 0, 10), ~76 deg angle (outside cone)
        assert!(grid.insert(4, Vec3Fix::from_f64(40.0, 0.0, 10.0)).is_ok());
        // Entity 5: Ahead at 60m (outside max range 50m)
        assert!(grid.insert(5, Vec3Fix::from_f64(0.0, 0.0, 60.0)).is_ok());

        let mut output = [0u32; 16];
        let res = grid.query_vision_cone_batched_16x(
            observer,
            forward,
            cos_half_sq,
            max_range_sq,
            personal_space_sq,
            &mut output,
        );

        assert_eq!(res.written, 2);
        assert_eq!(res.total_matches, 2);
        let matched = &output[..res.written];
        assert!(matched.contains(&1));
        assert!(matched.contains(&2));
        assert!(!matched.contains(&3));
        assert!(!matched.contains(&4));
        assert!(!matched.contains(&5));
    }
}
