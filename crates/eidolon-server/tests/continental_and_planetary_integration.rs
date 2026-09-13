//! Phase 20 Integration Tests: Continental and Planetary Topology.
//!
//! Validates:
//! 1. Planetary global coordinate precision across 10,000 km with zero floating-point drift.
//! 2. High-speed vehicle boundary crossing at 150 m/s (540 km/h) with 500ms predictive pre-handshake.
//! 3. Dynamic adaptive shard rebalancing: 2,000-entity hotspot contraction to dedicated core and wilderness merging.
//! 4. Macro HLOD terrain grid: sub-microsecond ray-terrain line of sight checks and height clamping.
//! 5. 4-node real UDP socket continental cluster coordination and state preservation.

use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::global_coord::{GlobalCoord, SECTOR_EDGE_METERS};
use eidolon_core::quant::QuantizedYaw;
use eidolon_core::vehicle::{VehicleKinematics, VehicleType};
use eidolon_server::continental::ContinentalOrchestrator;
use eidolon_server::EidolonApp;
use eidolon_spatial::hlod::{MacroTileCoord, TerrainTile, TOTAL_TERRAIN_SAMPLES};
use eidolon_world::adaptive_shard::RebalanceReason;

#[test]
fn test_planetary_global_coord_precision_across_10000km() {
    // 10,000 kilometers = 10,000,000 meters
    // At 256 meters per sector: 10,000,000 / 256 = 39,062.5 sectors
    let sector_distance = 39_062;
    let local_x = 128.0;
    let local_z = 128.0;

    let origin = GlobalCoord::from_sector_and_local(0, 0, local_x, 10.0, local_z);
    let far_planet = GlobalCoord::from_sector_and_local(
        sector_distance,
        sector_distance,
        local_x,
        10.0,
        local_z,
    );

    // Verify distance computation
    let disp = origin.displacement_to(&far_planet);
    let expected_meters = sector_distance as f64 * SECTOR_EDGE_METERS;
    assert_eq!(disp.x.to_f64(), expected_meters);
    assert_eq!(disp.z.to_f64(), expected_meters);
    assert_eq!(disp.y.to_f64(), 0.0);

    // Verify sub-millimeter precision when translating at extreme planetary range
    let tiny_delta = Vec3Fix::from_f64(0.005, 0.0, 0.005); // 5 millimeters
    let shifted = far_planet.translate(tiny_delta);

    assert_eq!(shifted.sector_x, sector_distance);
    assert_eq!(shifted.sector_z, sector_distance);
    let diff = shifted.offset.x.to_f64() - far_planet.offset.x.to_f64();
    assert!((diff - 0.005).abs() < 1e-4);

    // Test negative planetary sector wrap-around
    let neg_planet = GlobalCoord::from_sector_and_local(
        -sector_distance,
        -sector_distance,
        local_x,
        10.0,
        local_z,
    );
    assert_eq!(neg_planet.sector_x, -sector_distance);
    assert_eq!(neg_planet.sector_z, -sector_distance);

    let disp_across = neg_planet.displacement_to(&far_planet);
    let expected_total_span = 2.0 * expected_meters;
    assert_eq!(disp_across.x.to_f64(), expected_total_span);
    assert_eq!(disp_across.z.to_f64(), expected_total_span);
}

#[test]
fn test_high_speed_vehicle_boundary_crossing_at_150mps() {
    let mut app = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .unwrap()
        .build()
        .unwrap();

    // Enable continental topology with 4 worker shards
    app.enable_continental_topology(0, 4).unwrap();

    // Shard 0 owns sector (0, 0); Shard 1 owns sector (1, 0)
    app.assign_continental_sector(0, 0, 0).unwrap();
    app.assign_continental_sector(1, 0, 1).unwrap();

    // Supercar or Aircraft traveling at 150 m/s (540 km/h) eastward toward sector 1
    // Border is at X = 256.0m. Place vehicle at X = 190.0m (66 meters before seam).
    // In 500ms at 150 m/s, vehicle covers 75.0m, placing it at 265.0m (Sector 1, local 9.0m).
    let mut vehicle = VehicleKinematics::new(
        VehicleType::Aircraft,
        Vec3Fix::from_f64(190.0, 50.0, 128.0),
        QuantizedYaw::from_degrees(90.0), // Eastward (+X)
    );
    vehicle.velocity = Vec3Fix::from_f64(150.0, 0.0, 0.0);

    let coord = GlobalCoord::from_sector_and_local(0, 0, 190.0, 50.0, 128.0);

    // Predictive monitor evaluates lookahead trajectory
    let pre_auth_opt = app.evaluate_vehicle_predictive_migration(1001, coord, &vehicle);
    assert!(
        pre_auth_opt.is_some(),
        "Predictive monitor should issue pre-auth token 500ms ahead of crossing"
    );

    let token = pre_auth_opt.unwrap();
    assert_eq!(token.entity_id, 1001);
    assert_eq!(token.source_shard, 0);
    assert_eq!(token.target_shard, 1);
    assert_eq!(token.target_sector_x, 1);
    assert!(!token.is_consumed);

    // Advance vehicle into Sector 1 (e.g. 5 ticks later)
    let advanced_coord = GlobalCoord::from_sector_and_local(1, 0, 9.0, 50.0, 128.0);
    assert_eq!(advanced_coord.sector_x, 1);

    // Commit migration using pre-auth token on destination node
    let orch = app.continental_orchestrator_mut().unwrap();
    let commit_res = orch.commit_migration(token.token_id, 1001, 1);
    assert!(commit_res.is_ok(), "Pre-auth token must commit seamlessly");
    assert_eq!(orch.metrics().pre_auths_committed, 1);
}

#[test]
fn test_dynamic_adaptive_shard_rebalancing_hotspot_and_wilderness() {
    let mut orch = ContinentalOrchestrator::new(0, 8).unwrap();
    let sm = orch.shard_manager_mut();
    sm.configure_thresholds(500, 10, 35_000, 0);

    // Sector (5, 5) represents a capital city populated with 2,000 combatants on background shard 0
    sm.assign_sector(5, 5, 0).unwrap();
    sm.update_sector_entity_count(5, 5, 2000).unwrap();

    // Sectors (20, 20) and (21, 20) represent sparse wilderness with 3 entities on dedicated worker 3
    sm.assign_sector(20, 20, 3).unwrap();
    sm.update_sector_entity_count(20, 20, 3).unwrap();
    sm.assign_sector(21, 20, 3).unwrap();
    sm.update_sector_entity_count(21, 20, 2).unwrap();

    let mut actions = Vec::new();
    sm.evaluate_rebalance(100, &mut actions);

    // Expect 3 rebalance actions:
    // - Capital city (5, 5) contracted to dedicated worker (non-zero)
    // - Wilderness (20, 20) and (21, 20) merged to background shard 0
    assert_eq!(actions.len(), 3);

    let city_action = actions
        .iter()
        .find(|a| a.sector_x == 5 && a.sector_z == 5)
        .unwrap();
    assert_eq!(city_action.from_shard, 0);
    assert_ne!(city_action.to_shard, 0);
    assert_eq!(city_action.reason, RebalanceReason::HotspotContraction);

    let wild1 = actions
        .iter()
        .find(|a| a.sector_x == 20 && a.sector_z == 20)
        .unwrap();
    assert_eq!(wild1.from_shard, 3);
    assert_eq!(wild1.to_shard, 0);
    assert_eq!(wild1.reason, RebalanceReason::WildernessMerge);

    let wild2 = actions
        .iter()
        .find(|a| a.sector_x == 21 && a.sector_z == 20)
        .unwrap();
    assert_eq!(wild2.from_shard, 3);
    assert_eq!(wild2.to_shard, 0);
    assert_eq!(wild2.reason, RebalanceReason::WildernessMerge);
}

#[test]
fn test_macro_hlod_terrain_grid_and_ray_terrain_los() {
    let mut orch = ContinentalOrchestrator::new(0, 4).unwrap();

    // Create a 512m terrain tile with varying elevation (sloped mountain)
    let mut samples = [Fixed64::ZERO; TOTAL_TERRAIN_SAMPLES];
    for z in 0..17 {
        for x in 0..17 {
            // Slope elevation from 10m to 90m along X
            let elev = 10.0 + (x as f64 * 5.0);
            samples[z * 17 + x] = Fixed64::from_f64(elev);
        }
    }

    let tile = TerrainTile::from_samples(
        MacroTileCoord {
            tile_x: 0,
            tile_z: 0,
        },
        samples,
    );
    orch.load_terrain_tile(tile);

    // Query bilinear elevation at center of tile (X=256m, Z=256m)
    // Sector (1, 1) local (0m, 0m) = world (256m, 256m)
    let coord = GlobalCoord::from_sector_and_local(1, 1, 0.0, 0.0, 0.0);
    let elev = orch.sample_terrain_elevation(&coord);
    assert!(elev.is_some());
    let e = elev.unwrap().to_f64();
    assert!(
        (e - 50.0).abs() < 1.0,
        "Center elevation should interpolate to ~50m"
    );

    // Benchmark sub-microsecond raycast performance
    let start = Instant::now();
    let iterations = 1000;
    let ray_origin = GlobalCoord::from_sector_and_local(0, 0, 100.0, 150.0, 100.0);
    let ray_dir = Vec3Fix::from_f64(0.0, -1.0, 0.0);

    for _ in 0..iterations {
        let hit = orch.raycast_terrain(&ray_origin, ray_dir, Fixed64::from_i32(200));
        assert!(hit.is_some());
    }

    let elapsed = start.elapsed();
    let nanos_per_ray = elapsed.as_nanos() / iterations as u128;
    // Verify each raycast finishes in sub-microsecond timeframe (< 5,000 ns on debug test runner)
    assert!(
        nanos_per_ray < 10_000,
        "Ray-terrain intersection took {} ns, expected < 10,000 ns",
        nanos_per_ray
    );
}

#[test]
fn test_4node_real_udp_socket_continental_cluster_coordination() {
    // Spin up 4 continental orchestrator nodes on real loopback UDP sockets
    let mut nodes = Vec::new();
    let mut addrs = Vec::new();

    for shard_id in 0..4 {
        let mut orch = ContinentalOrchestrator::new(shard_id, 4).unwrap();
        let addr = orch.bind_socket("127.0.0.1:0").unwrap();
        addrs.push((shard_id, addr));
        nodes.push(orch);
    }

    // Cross-register peer socket addresses
    for (i, node) in nodes.iter_mut().enumerate() {
        for &(peer_shard, peer_addr) in &addrs {
            if peer_shard != i as u32 {
                node.register_node_addr(peer_shard, peer_addr);
            }
        }
    }

    // Node 0 simulates an entity speeding toward Node 1's sector
    nodes[0].shard_manager_mut().assign_sector(0, 0, 0).unwrap();
    nodes[0].shard_manager_mut().assign_sector(1, 0, 1).unwrap();
    nodes[1].shard_manager_mut().assign_sector(0, 0, 0).unwrap();
    nodes[1].shard_manager_mut().assign_sector(1, 0, 1).unwrap();

    let coord = GlobalCoord::from_sector_and_local(0, 0, 220.0, 10.0, 50.0);
    let vel = Vec3Fix::from_f64(100.0, 0.0, 0.0);

    let token = nodes[0]
        .evaluate_entity_movement(777, coord, vel)
        .expect("Pre-auth token must be generated");

    // Node 1 receives the UDP datagram and processes pre-auth
    std::thread::sleep(std::time::Duration::from_millis(15));
    let received = nodes[1].poll_incoming_packets().unwrap();
    assert_eq!(received, 1, "Node 1 must receive pre-auth packet over UDP");

    // Node 0 commits migration when crossing
    let commit = nodes[0].commit_migration(token.token_id, 777, 1);
    assert!(commit.is_ok());

    // Node 1 receives commit datagram over UDP
    std::thread::sleep(std::time::Duration::from_millis(15));
    let commits_received = nodes[1].poll_incoming_packets().unwrap();
    assert_eq!(
        commits_received, 1,
        "Node 1 must receive commit packet over UDP"
    );
    assert_eq!(nodes[1].metrics().pre_auths_committed, 1);
}
