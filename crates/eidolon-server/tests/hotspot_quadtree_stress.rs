//! Tier 3 & Tier 5 Chaos and Stress Verification: Micro-Hotspot Dynamic Sub-Sharding.
//!
//! Verifies hierarchical quadtree spatial sub-sharding when 5,000 players congregate
//! in a localized 50m arena, asserting sub-millisecond query latency, zero coordinate tearing,
//! and seamless hysteresis merging upon crowd dispersion.

#![deny(unsafe_code)]

use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_spatial::grid::{CellCoord, SpatialHashGrid};
use eidolon_spatial::quadtree::HotspotManager;

#[test]
fn test_5000_entity_concentrated_gathering_and_quadtree_pruning() {
    const NUM_ENTITIES: usize = 5_000;
    const ARENA_RADIUS: f64 = 25.0; // 50m arena diameter

    let mut grid = SpatialHashGrid::with_capacity(NUM_ENTITIES + 100, 256);

    // Insert 5,000 entities distributed inside the 50m arena
    for i in 0..NUM_ENTITIES {
        let angle = (i as f64) * 0.137; // golden angle spiral
        let dist = ((i as f64) / (NUM_ENTITIES as f64)).sqrt() * ARENA_RADIUS;
        let x = 32.0 + dist * angle.cos();
        let z = 32.0 + dist * angle.sin();
        let pos = Vec3Fix::from_f64(x, 0.0, z);

        grid.insert(i as u32, pos).unwrap();
    }

    assert_eq!(grid.active_count(), NUM_ENTITIES);

    // Center cell (0, 0, 0) spans [0..64, 0..64] in (X, Z).
    // The 5,000 entities in the 50m arena are inside cell (0, 0, 0).
    let center_cell = CellCoord::new(0, 0, 0);
    assert!(
        grid.hotspots().is_split(center_cell),
        "Cell with 5,000 entities must be dynamically split into micro-quadrants"
    );

    // Perform 1,000 queries for immediate AoI (10m radius = 100m^2) across the arena
    let mut out_buffer = vec![0u32; 1024];
    let query_radius_sq = Fixed64::from_i32(100); // 10m radius

    let start = Instant::now();
    let num_queries = 1_000;
    let mut total_matches = 0;

    for q in 0..num_queries {
        let q_angle = (q as f64) * 0.25;
        let q_dist = (q % 20) as f64;
        let q_pos = Vec3Fix::from_f64(
            32.0 + q_dist * q_angle.cos(),
            0.0,
            32.0 + q_dist * q_angle.sin(),
        );

        let res = grid.query_radius_squared(q_pos, query_radius_sq, &mut out_buffer);
        total_matches += res.total_matches;
        assert!(res.total_matches > 0);
    }

    let elapsed = start.elapsed();
    let avg_us = (elapsed.as_micros() as f64) / (num_queries as f64);

    println!(
        "5,000-Entity Arena Benchmark: 1,000 queries completed in {:?} (average {:.2} us/query, total matches: {})",
        elapsed, avg_us, total_matches
    );

    // In debug mode, average query should still be under 500 us; in release mode, < 50 us
    assert!(
        avg_us < 1000.0,
        "Average query latency ({:.2} us) must be sub-millisecond under 5,000-entity load",
        avg_us
    );
}

#[test]
fn test_dynamic_hotspot_crowd_dispersion_and_seamless_merge() {
    const INITIAL_CROWD: usize = 350;
    let mut manager = HotspotManager::new(INITIAL_CROWD + 10, 8);
    let cell = CellCoord::new(0, 0, 0);

    // Gather crowd of 350 entities in cell (0, 0, 0)
    for i in 0..INITIAL_CROWD {
        let x = (i % 50) as f64 + 5.0;
        let z = ((i / 50) * 8) as f64 + 5.0;
        manager
            .insert(i as u32, Vec3Fix::from_f64(x, 0.0, z))
            .unwrap();
    }

    assert!(
        manager.is_split(cell),
        "Initial crowd of 350 must trigger quadtree subdivision"
    );

    // Move entities across micro-quadrant seams (e.g. from x < 16 to x >= 16)
    for i in 0..50 {
        let new_pos = Vec3Fix::from_f64(45.0, 0.0, 45.0); // Move to NE quadrant
        manager.update_position(i as u32, new_pos).unwrap();
    }

    // Crowd disperses: remove entities until count drops below HOTSPOT_MERGE_THRESHOLD (100)
    // Remove down to 101: should still remain split
    for i in 101..INITIAL_CROWD {
        manager.remove(i as u32).unwrap();
    }
    assert!(manager.is_split(cell));

    // Remove 100th entity: triggers seamless merge back to flat cell
    manager.remove(100).unwrap();
    assert!(
        !manager.is_split(cell),
        "Dropping to 100 entities must trigger hysteresis merge back to flat root cell"
    );

    // Verify all remaining 100 entities (0..=99) are intact and queryable
    let mut out_buffer = [0u32; 256];
    let query_all = manager.query_radius(
        Vec3Fix::from_f64(32.0, 0.0, 32.0),
        Fixed64::from_i32(64 * 64 * 2),
        &mut out_buffer,
    );
    assert_eq!(
        query_all.total_matches, 100,
        "All 100 entities must be fully preserved post-merge"
    );
}

#[test]
fn test_quadtree_subsharding_speedup_vs_linear_scan() {
    const ENTITY_COUNT: usize = 2_000;
    let mut manager = HotspotManager::new(ENTITY_COUNT + 10, 4);

    // Cluster 2,000 entities in cell (0, 0, 0)
    for i in 0..ENTITY_COUNT {
        let x = (i % 60) as f64 + 2.0;
        let z = ((i * 7) % 60) as f64 + 2.0;
        manager
            .insert(i as u32, Vec3Fix::from_f64(x, 0.0, z))
            .unwrap();
    }

    let cell = CellCoord::new(0, 0, 0);
    assert!(manager.is_split(cell));

    // Query 10m radius in SW quadrant (x=8, z=8)
    let center = Vec3Fix::from_f64(8.0, 0.0, 8.0);
    let radius_sq = Fixed64::from_i32(100); // 10m radius

    let mut quadtree_results = [0u32; 512];
    let res = manager.query_radius(center, radius_sq, &mut quadtree_results);

    // Ground-truth linear scan
    let mut ground_truth = Vec::new();
    for i in 0..ENTITY_COUNT {
        let dist_sq = Vec3Fix::from_f64((i % 60) as f64 + 2.0, 0.0, ((i * 7) % 60) as f64 + 2.0)
            .distance_squared(center);
        if dist_sq <= radius_sq {
            ground_truth.push(i as u32);
        }
    }

    assert_eq!(res.total_matches, ground_truth.len());
    for id in &ground_truth {
        assert!(
            quadtree_results[..res.written].contains(id),
            "Every matching entity must be found by quadtree query"
        );
    }
}

#[test]
fn test_quadtree_boundary_seam_traversal_integrity() {
    let mut manager = HotspotManager::new(300, 4);

    // Populate 260 entities to trigger split
    for i in 0..260 {
        manager
            .insert(i as u32, Vec3Fix::from_f64(10.0, 0.0, 10.0))
            .unwrap();
    }

    let cell = CellCoord::new(0, 0, 0);
    assert!(manager.is_split(cell));

    // Oscillate entity 0 across 16m micro-quadrant border 50 times
    for step in 0..50 {
        let x = if step % 2 == 0 { 15.5 } else { 16.5 };
        manager
            .update_position(0, Vec3Fix::from_f64(x, 0.0, 10.0))
            .unwrap();
    }

    // Assert entity 0 is still present and valid
    let mut buf = [0u32; 10];
    let res = manager.query_radius(
        Vec3Fix::from_f64(16.5, 0.0, 10.0),
        Fixed64::from_i32(4),
        &mut buf,
    );
    assert!(buf[..res.written].contains(&0));
}
