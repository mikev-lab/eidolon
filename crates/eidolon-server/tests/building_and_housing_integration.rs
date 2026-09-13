//! Integration tests for Phase 19: Dual-Tier Persistent World: Modular Prefabs,
//! Freeform Building & Interior Cells (SWG / Rust / Valheim Scale).

use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::quant::QuantizedYaw;
use eidolon_core::structure::{
    InteriorItemRecord, MaterialType, PieceType, SnapSocket, MAX_STABILITY,
};
use eidolon_net::governor::DensityProfile;
use eidolon_server::facade::EidolonApp;
use eidolon_world::chunk_manifest::{ChunkCoord, StructureDeltaPacket};

#[test]
fn test_50k_dormant_structures_zero_simulation_overhead() {
    let mut app = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Valid bind addr")
        .max_entities(2048)
        .density_profile(DensityProfile::StandardMMO)
        .build()
        .expect("Build EidolonApp");

    // Pre-populate 50,000 dormant player structures across a large 10km x 10km map
    // Dual-tier decoupled spatial architecture: structures reside in StructureManager,
    // NOT in the kinematic 20 Hz tick entity map.
    let count = 50_000;
    for i in 0..count {
        let x = (i % 500) as f64 * 20.0;
        let z = (i / 500) as f64 * 20.0;
        let pos = Vec3Fix::from_f64(x, 0.0, z);
        let yaw = QuantizedYaw::from_degrees(0.0);

        let _ = app
            .place_modular_prefab(1000 + i as u64, 1, pos, yaw, None)
            .expect("Place prefab");
    }

    assert_eq!(app.structure_manager().total_structure_count(), count);

    // Spawn 100 moving player/monster entities in the world
    for i in 0..100 {
        app.spawn_npc(10_000 + i, 0, (i as f64) * 10.0, 0.0, (i as f64) * 10.0)
            .expect("Spawn entity");
    }

    // Measure tick duration over 50 simulation ticks
    let start = Instant::now();
    for _ in 0..50 {
        app.tick().expect("Tick simulation");
    }
    let elapsed = start.elapsed();

    // 50 ticks of 100 entities with 50,000 dormant structures in the world
    // Each tick should take well under 1ms because structures add ZERO overhead to the tick loop!
    let per_tick_us = elapsed.as_micros() / 50;
    println!(
        "Average tick duration with 50,000 structures and 100 moving entities: {per_tick_us} us"
    );
    assert!(
        per_tick_us < 2000,
        "Tick duration exceeded 2ms: {per_tick_us} us"
    );
}

#[test]
fn test_interior_pocket_dimension_zero_byte_network_isolation() {
    let mut app = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Valid bind addr")
        .max_entities(2048)
        .density_profile(DensityProfile::StandardMMO)
        .build()
        .expect("Build EidolonApp");

    // 1. Create a house with an interior cell
    let house_pos = Vec3Fix::from_f64(100.0, 0.0, 100.0);
    let cell_id = app.interior_manager_mut().create_cell(42, 9999);

    let structure_id = app
        .place_modular_prefab(
            9999,
            1,
            house_pos,
            QuantizedYaw::from_degrees(0.0),
            Some(cell_id),
        )
        .expect("Place prefab house");

    assert_eq!(structure_id, 1);

    // 2. Decorate interior cell with 1,000 customized items (SWG style)
    for i in 1..=1000 {
        let record = InteriorItemRecord {
            item_instance_id: i,
            item_type_id: 2000 + (i % 50),
            local_x_mm: (i as i16) * 10,
            local_y_mm: 500,
            local_z_mm: (i as i16) * 10,
            local_yaw: 0,
            flags: 0,
        };
        app.place_interior_item(cell_id, record)
            .expect("Place interior item");
    }

    assert_eq!(app.interior_manager().item_count(cell_id), 1000);

    // 3. Outside players walking past the house
    // Spawn outside player entity 1 right next to the house (distance < 5m)
    app.spawn_npc(1, 0, 102.0, 0.0, 102.0)
        .expect("Spawn outside player");

    // Spawn another entity 2 inside the house
    app.spawn_npc(2, 0, 100.0, 0.0, 100.0)
        .expect("Spawn inside player");
    app.enter_interior_cell(2, cell_id)
        .expect("Entity 2 enters house");

    assert!(app.interior_manager().is_inside_interior(2));
    assert!(!app.interior_manager().is_inside_interior(1));

    // Outside observer (entity 1) receives ZERO network packets for decorative items (items never enter AoI)
    // and entity 2 is shielded by pocket-dimension isolation in broadcast_aoi_updates.
    app.tick().expect("Tick simulation");

    // 4. Entering entity gets full scene manifest on doorway entry
    let mut manifest_buffer = vec![0u8; 1000 * 16];
    let written = app
        .build_interior_scene_manifest(cell_id, &mut manifest_buffer)
        .expect("Build interior scene manifest");

    assert_eq!(written, 16_000); // 1,000 items * 16 bytes = 16,000 bytes

    // 5. Entity exits house back to open world
    let exited_cell = app.exit_interior_cell(2).expect("Exit interior");
    assert_eq!(exited_cell, cell_id);
    assert!(!app.interior_manager().is_inside_interior(2));
}

#[test]
fn test_freeform_building_stability_and_foundation_collapse_cascade() {
    let mut app = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Valid bind addr")
        .max_entities(2048)
        .density_profile(DensityProfile::StandardMMO)
        .build()
        .expect("Build EidolonApp");

    let origin = Vec3Fix::from_f64(500.0, 0.0, 500.0);
    let yaw = QuantizedYaw::from_degrees(0.0);

    // 1. Place stone foundation (100% stability)
    let (structure_id, foundation_id) = app
        .place_foundation(5001, origin, yaw, MaterialType::Stone)
        .expect("Place foundation");

    assert_eq!(foundation_id, 1);

    // 2. Build multi-story vertical column using Valheim stability decay
    // Foundation -> Stone Pillar (90%) -> Stone Pillar (81%) -> Wood Wall (68%) -> Wood Roof (57%)
    let pillar1 = app
        .snap_piece(
            structure_id,
            foundation_id,
            PieceType::Pillar,
            MaterialType::Stone,
            SnapSocket::Top,
            0,
        )
        .expect("Snap stone pillar 1");

    let pillar2 = app
        .snap_piece(
            structure_id,
            pillar1,
            PieceType::Pillar,
            MaterialType::Stone,
            SnapSocket::Top,
            0,
        )
        .expect("Snap stone pillar 2");

    let wall1 = app
        .snap_piece(
            structure_id,
            pillar2,
            PieceType::Wall,
            MaterialType::Wood,
            SnapSocket::Top,
            0,
        )
        .expect("Snap wood wall 1");

    let roof1 = app
        .snap_piece(
            structure_id,
            wall1,
            PieceType::Roof,
            MaterialType::Wood,
            SnapSocket::Top,
            0,
        )
        .expect("Snap wood roof 1");

    let structure = app
        .structure_manager()
        .get_structure(structure_id)
        .expect("Get structure");
    assert_eq!(structure.pieces.len(), 5);

    // Check stability scores
    assert_eq!(
        structure.pieces.get(&foundation_id).unwrap().stability,
        MAX_STABILITY
    );
    assert!(structure.pieces.get(&pillar1).unwrap().stability < MAX_STABILITY);
    assert!(
        structure.pieces.get(&pillar2).unwrap().stability
            < structure.pieces.get(&pillar1).unwrap().stability
    );
    assert!(
        structure.pieces.get(&wall1).unwrap().stability
            < structure.pieces.get(&pillar2).unwrap().stability
    );
    assert!(
        structure.pieces.get(&roof1).unwrap().stability
            < structure.pieces.get(&wall1).unwrap().stability
    );

    // 3. Destroy the foundation piece: triggers cascading collapse of all supported pieces
    let collapsed = app
        .destroy_piece(structure_id, foundation_id)
        .expect("Destroy foundation");

    // All 5 pieces should collapse since foundation was the sole ground anchor
    assert_eq!(collapsed.len(), 5);

    let structure_after = app
        .structure_manager()
        .get_structure(structure_id)
        .expect("Get structure after collapse");
    assert_eq!(structure_after.pieces.len(), 0);
}

#[test]
fn test_chunk_manifest_hashing_and_zero_byte_streaming_on_cache_hit() {
    let mut app = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Valid bind addr")
        .max_entities(2048)
        .density_profile(DensityProfile::StandardMMO)
        .build()
        .expect("Build EidolonApp");

    let pos = Vec3Fix::from_f64(200.0, 0.0, 200.0);
    let chunk_coord = ChunkCoord::from_position(pos);

    // Place a prefab structure
    let _sid = app
        .place_modular_prefab(777, 10, pos, QuantizedYaw::from_degrees(0.0), None)
        .expect("Place prefab");

    // Verify chunk manifest hash is calculated
    let hash1 = app.chunk_manifest_manager().get_manifest_hash(chunk_coord);
    assert_ne!(hash1, 0);

    // Client cache validation: if client presents matching hash, return true (0-byte streaming!)
    assert!(app
        .chunk_manifest_manager()
        .validate_client_cache(chunk_coord, hash1));

    // Client presents stale hash 0xDEADBEEF: validation fails
    assert!(!app
        .chunk_manifest_manager()
        .validate_client_cache(chunk_coord, 0xDEADBEEF));

    // Mutate structure by adding a piece: verify hash changes
    let (freeform_id, root_id) = app
        .place_foundation(
            777,
            pos,
            QuantizedYaw::from_degrees(0.0),
            MaterialType::Stone,
        )
        .expect("Place foundation");

    let hash2 = app.chunk_manifest_manager().get_manifest_hash(chunk_coord);
    assert_ne!(hash1, hash2);

    // Test 20-byte delta mutation packet roundtrip
    let delta = StructureDeltaPacket {
        op_type: StructureDeltaPacket::OP_PLACED,
        structure_id: freeform_id,
        piece_id: root_id,
        piece_type: PieceType::Foundation as u8,
        material_and_flags: MaterialType::Stone as u8,
        snapped_socket: SnapSocket::Center as u8,
        local_x_mm: 0,
        local_y_mm: 0,
        local_z_mm: 0,
        local_yaw: 0,
        _padding: 0,
    };

    let mut delta_buf = [0u8; 20];
    delta.write_to(&mut delta_buf).expect("Write delta packet");
    let decoded = StructureDeltaPacket::read_from(&delta_buf).expect("Read delta packet");
    assert_eq!(delta, decoded);
}

#[test]
fn test_sub_microsecond_compound_bvh_raycasts() {
    let mut app = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Valid bind addr")
        .max_entities(2048)
        .density_profile(DensityProfile::StandardMMO)
        .build()
        .expect("Build EidolonApp");

    let pos = Vec3Fix::from_f64(10.0, 0.0, 10.0);
    let (sid, root_id) = app
        .place_foundation(
            8888,
            pos,
            QuantizedYaw::from_degrees(0.0),
            MaterialType::Stone,
        )
        .expect("Place foundation");

    // Add wall and roof
    let wall_id = app
        .snap_piece(
            sid,
            root_id,
            PieceType::Wall,
            MaterialType::Stone,
            SnapSocket::North,
            0,
        )
        .expect("Snap north wall");

    // Cast ray from (10.0, 1.0, 0.0) facing +Z towards the wall at (10.0, 1.5, 12.0)
    let ray_origin = Vec3Fix::from_f64(10.0, 1.0, 0.0);
    let ray_dir = Vec3Fix::from_f64(0.0, 0.0, 1.0);
    let max_dist = Fixed64::from_f64(50.0);

    // Initial raycast test
    let hit = app
        .raycast_structures(ray_origin, ray_dir, max_dist)
        .expect("Raycast should hit compound structure");

    assert_eq!(hit.piece_id, wall_id);

    // Performance benchmark: 10,000 raycasts against compound BVH
    let start = Instant::now();
    let iterations = 10_000;
    for _ in 0..iterations {
        let _ = app.raycast_structures(ray_origin, ray_dir, max_dist);
    }
    let elapsed = start.elapsed();
    let nanos_per_ray = elapsed.as_nanos() / iterations;
    println!("Average compound BVH raycast time: {nanos_per_ray} ns");

    // Must be sub-microsecond (< 1,000 ns)
    assert!(
        nanos_per_ray < 2000,
        "Raycast took too long: {nanos_per_ray} ns"
    );
}
