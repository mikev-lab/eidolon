//! Tier 3 Test Matrix: Concurrency, spatial saturation stress, and boundary seam oscillations.

use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_spatial::aoi::{
    AoIScheduler, LoadSheddingLevel, ObserverInterestSet, VisibilityEvent, MAX_AOI_RADIUS_SQ,
};
use eidolon_spatial::grid::{CellCoord, SpatialHashGrid};
use eidolon_spatial::tier::FrequencyTier;

#[test]
fn test_cell_density_saturation_1000_entities() {
    let mut grid = SpatialHashGrid::with_capacity(2000, 2048);
    let cell_center = Vec3Fix::from_f64(30.0, 14.0, 2.0);

    // Pack 1,000 entities inside a single 64m cell [0..64] x [0..32] x [0..64]
    for i in 0..1000u32 {
        let x = (i % 31) as f64 * 2.0;
        let y = ((i / 31) % 15) as f64 * 2.0;
        let z = ((i / 465) % 31) as f64 * 2.0;

        let pos = Vec3Fix::from_f64(x, y, z);
        assert!(grid.insert(i, pos).is_ok());
    }

    assert_eq!(grid.active_count(), 1000);
    assert_eq!(
        grid.cell_entity_count(CellCoord::new(0, 0, 0)),
        1000,
        "All 1,000 entities must reside in cell (0, 0, 0)"
    );

    // Execute query within 20m of center (radius_sq = 400 m^2)
    let mut output_buffer = [0u32; 1000];
    let query_radius_sq = Fixed64::from_i32(400);

    let start = Instant::now();
    let res = grid.query_radius_squared(cell_center, query_radius_sq, &mut output_buffer);
    let elapsed = start.elapsed();

    // Profile-aware execution budget: <100µs in release profile, <2,000µs in debug profile on shared cloud runners
    let max_micros = if cfg!(debug_assertions) { 2000 } else { 100 };
    assert!(
        elapsed.as_micros() < max_micros,
        "Spatial query on 1,000 packed entities threshold exceeded, took {:?}",
        elapsed
    );
    assert!(res.written > 0, "Query must find clustered entities");

    // Verify correctness: all matched entities must be <= 20m from center
    for &id in &output_buffer[..res.written] {
        let pos = grid.get_position(id).expect("Active entity position");
        assert!(
            pos.distance_squared(cell_center) <= query_radius_sq,
            "Matched entity must lie within query radius"
        );
    }
}

#[test]
fn test_grid_seam_boundary_oscillation_500_entities() {
    let mut grid = SpatialHashGrid::with_capacity(1000, 1024);

    // Insert 500 entities near cell boundary X = 64.0m
    for i in 0..500u32 {
        let z = (i % 50) as f64;
        let pos = Vec3Fix::from_f64(63.9, 5.0, z);
        assert!(grid.insert(i, pos).is_ok());
    }

    assert_eq!(grid.active_count(), 500);

    // Rapidly oscillate 500 entities across the seam between cell 0 and cell 1 for 50 cycles
    for cycle in 0..50 {
        let x = if cycle % 2 == 0 { 64.1 } else { 63.9 };
        let expected_cell_x = if cycle % 2 == 0 { 1 } else { 0 };

        for i in 0..500u32 {
            let z = (i % 50) as f64;
            let new_pos = Vec3Fix::from_f64(x, 5.0, z);
            let crossed = grid.update_position(i, new_pos).expect("Update position");
            assert!(crossed, "Oscillation must cross cell boundary");

            let cell = grid.get_cell(i).expect("Entity cell");
            assert_eq!(
                cell.x, expected_cell_x,
                "Entity cell index must reflect current oscillation side"
            );
        }

        assert_eq!(
            grid.active_count(),
            500,
            "Active count must remain invariant during seam oscillation"
        );
    }
}

#[test]
fn test_observer_visibility_lifecycle_and_hysteresis() {
    let mut grid = SpatialHashGrid::with_capacity(200, 256);
    let mut observer_interest = ObserverInterestSet::with_capacity(100);

    // Observer initially at (0, 0, 0)
    let observer_pos = Vec3Fix::ZERO;

    // Entity 1: at 8m (Immediate tier)
    // Entity 2: at 25m (Mid tier)
    // Entity 3: at 60m (Outside visible Horizon > 50m)
    assert!(grid.insert(1, Vec3Fix::from_f64(8.0, 0.0, 0.0)).is_ok());
    assert!(grid.insert(2, Vec3Fix::from_f64(25.0, 0.0, 0.0)).is_ok());
    assert!(grid.insert(3, Vec3Fix::from_f64(60.0, 0.0, 0.0)).is_ok());

    let mut query_buf = [0u32; 32];
    let query_res = grid.query_radius_squared(observer_pos, MAX_AOI_RADIUS_SQ, &mut query_buf);
    let candidate_count = query_res.written;
    assert_eq!(
        candidate_count, 2,
        "Entities 1 and 2 must be within 50m AoI"
    );

    let mut events = [VisibilityEvent::Exit { entity_id: 0 }; 16];
    let event_count = observer_interest.update_visibility(
        &grid,
        observer_pos,
        &query_buf[..candidate_count],
        &mut events,
    );

    assert_eq!(event_count, 2);
    assert!(events[..event_count].contains(&VisibilityEvent::Enter {
        entity_id: 1,
        tier: FrequencyTier::Immediate,
    }));
    assert!(events[..event_count].contains(&VisibilityEvent::Enter {
        entity_id: 2,
        tier: FrequencyTier::Mid,
    }));

    // Move Entity 1 from 8m to 10m: within hysteresis deadband [81, 121], remains Immediate
    assert_eq!(
        grid.update_position(1, Vec3Fix::from_f64(10.0, 0.0, 0.0)),
        Ok(false)
    );
    let query_res = grid.query_radius_squared(observer_pos, MAX_AOI_RADIUS_SQ, &mut query_buf);
    let candidate_count = query_res.written;
    let event_count = observer_interest.update_visibility(
        &grid,
        observer_pos,
        &query_buf[..candidate_count],
        &mut events,
    );
    assert_eq!(
        event_count, 0,
        "Entity 1 within hysteresis deadband must not emit tier change event"
    );

    // Move Entity 1 to 12m: exceeds 121 m^2, demotes to Mid tier
    assert_eq!(
        grid.update_position(1, Vec3Fix::from_f64(12.0, 0.0, 0.0)),
        Ok(false)
    );
    let query_res = grid.query_radius_squared(observer_pos, MAX_AOI_RADIUS_SQ, &mut query_buf);
    let candidate_count = query_res.written;
    let event_count = observer_interest.update_visibility(
        &grid,
        observer_pos,
        &query_buf[..candidate_count],
        &mut events,
    );
    assert_eq!(event_count, 1);
    assert_eq!(
        events[0],
        VisibilityEvent::TierChange {
            entity_id: 1,
            tier: FrequencyTier::Mid,
        }
    );

    // Move Entity 2 beyond 50m (to 55m): exits visible Horizon
    assert_eq!(
        grid.update_position(2, Vec3Fix::from_f64(55.0, 0.0, 0.0)),
        Ok(false)
    );
    let query_res = grid.query_radius_squared(observer_pos, MAX_AOI_RADIUS_SQ, &mut query_buf);
    let candidate_count = query_res.written;
    let event_count = observer_interest.update_visibility(
        &grid,
        observer_pos,
        &query_buf[..candidate_count],
        &mut events,
    );
    assert_eq!(event_count, 1);
    assert_eq!(events[0], VisibilityEvent::Exit { entity_id: 2 });
}

#[test]
fn test_load_shedding_degradation_levels() {
    let mut scheduler = AoIScheduler::new();

    // Normal operation
    assert_eq!(scheduler.load_shedding(), LoadSheddingLevel::None);
    let mut immediate_dispatches = 0;
    let mut mid_dispatches = 0;

    for tick in 1..=20 {
        if scheduler.should_replicate(FrequencyTier::Immediate, tick, false) {
            immediate_dispatches += 1;
        }
        if scheduler.should_replicate(FrequencyTier::Mid, tick, false) {
            mid_dispatches += 1;
        }
    }

    // Over 20 ticks (1 second): Immediate dispatches 10 times (10 Hz), Mid dispatches 2 times (2 Hz)
    assert_eq!(immediate_dispatches, 10);
    assert_eq!(mid_dispatches, 2);

    // Level 1 load shedding: Mid throttled from 2 Hz to 1 Hz
    scheduler.set_load_shedding(LoadSheddingLevel::Level1);
    let mut mid_level1_dispatches = 0;
    for tick in 1..=20 {
        if scheduler.should_replicate(FrequencyTier::Mid, tick, false) {
            mid_level1_dispatches += 1;
        }
    }
    assert_eq!(
        mid_level1_dispatches, 1,
        "Level 1 shedding must throttle Mid to 1 Hz (1 dispatch per 20 ticks)"
    );

    // Level 2 load shedding: Mid dropped completely, Immediate preserved
    scheduler.set_load_shedding(LoadSheddingLevel::Level2);
    let mut immediate_level2_dispatches = 0;
    let mut mid_level2_dispatches = 0;
    for tick in 1..=20 {
        if scheduler.should_replicate(FrequencyTier::Immediate, tick, false) {
            immediate_level2_dispatches += 1;
        }
        if scheduler.should_replicate(FrequencyTier::Mid, tick, false) {
            mid_level2_dispatches += 1;
        }
    }
    assert_eq!(
        immediate_level2_dispatches, 10,
        "Immediate combat tier must be protected under Level 2"
    );
    assert_eq!(
        mid_level2_dispatches, 0,
        "Mid tier must be shed completely under Level 2"
    );
}
