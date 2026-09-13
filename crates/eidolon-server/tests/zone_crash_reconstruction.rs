//! Milestone 9.4: Disposable Zone Server Crash Reconstruction Tests.
//!
//! Simulates sudden zone worker crashes (SIGKILL) and verifies that replacement nodes
//! reconstruct identical zone state from periodic checkpoints and durable WAL replay.

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_world::reconstruction::{
    encode_position_payload, reconstruct_zone_from_wal, ZoneCheckpoint,
};
use eidolon_world::wal::{WalRecord, OP_ENTITY_DESPAWN, OP_ENTITY_SPAWN, OP_ENTITY_TRANSFORM};
use eidolon_world::zone::{SeamAxis, WorldZone, ZoneBounds};
use eidolon_world::ZoneId;

#[test]
fn test_milestone_9_4_disposable_zone_crash_reconstruction_sigkill() {
    let bounds = ZoneBounds::new(
        Fixed64::from_i32(0),
        Fixed64::from_i32(1000),
        Fixed64::from_i32(0),
        Fixed64::from_i32(1000),
        SeamAxis::EastWest,
        Fixed64::from_i32(984),
        Fixed64::from_i32(1000),
    );

    // Initial zone running before crash
    let mut initial_zone = WorldZone::new(ZoneId(10), bounds, None, true, 1024);

    // Baseline: 50 entities present at checkpoint
    let mut active_ids = Vec::with_capacity(50);
    for id in 1u32..=50u32 {
        let pos = Vec3Fix::new(
            Fixed64::from_i32((id * 10) as i32),
            Fixed64::from_i32(0),
            Fixed64::from_i32((id * 10) as i32),
        );
        initial_zone.insert_entity(id, pos).unwrap();
        active_ids.push(id);
    }

    // Capture Checkpoint at tick 1000, LSN 500
    let checkpoint = ZoneCheckpoint::capture(&initial_zone, 1000, 500, &active_ids)
        .expect("Checkpoint capture must succeed");
    assert_eq!(checkpoint.entities.len(), 50);

    // Subsequent mutations occurred between tick 1001 and 1050 (LSN 501 to 520)
    let mut wal_log = Vec::new();

    // 1. Entities 1..=5 despawn (leave zone)
    for id in 1u32..=5u32 {
        let lsn = 500 + id as u64;
        let rec = WalRecord::new(lsn, 1001, 1000 + id as u64, id, OP_ENTITY_DESPAWN, &[]).unwrap();
        wal_log.push(rec);
    }

    // 2. Entities 6..=10 move to new coordinates
    for id in 6u32..=10u32 {
        let lsn = 505 + (id - 5) as u64;
        let new_pos = Vec3Fix::new(
            Fixed64::from_i32((id * 20) as i32),
            Fixed64::from_i32(5),
            Fixed64::from_i32((id * 20) as i32),
        );
        let rec = WalRecord::new(
            lsn,
            1005,
            1000 + id as u64,
            id,
            OP_ENTITY_TRANSFORM,
            &encode_position_payload(new_pos),
        )
        .unwrap();
        wal_log.push(rec);
    }

    // 3. Entities 51..=55 spawn (new players enter zone)
    for id in 51u32..=55u32 {
        let lsn = 510 + (id - 50) as u64;
        let spawn_pos = Vec3Fix::new(
            Fixed64::from_i32((id * 10) as i32),
            Fixed64::from_i32(0),
            Fixed64::from_i32((id * 10) as i32),
        );
        let rec = WalRecord::new(
            lsn,
            1010,
            2000 + id as u64,
            id,
            OP_ENTITY_SPAWN,
            &encode_position_payload(spawn_pos),
        )
        .unwrap();
        wal_log.push(rec);
    }

    // CRASH OCCURS: Initial zone process is terminated (simulated SIGKILL)
    drop(initial_zone);

    // RECONSTRUCTION: Replacement zone server initializes from checkpoint and replays WAL
    let reconstructed_zone =
        reconstruct_zone_from_wal(&checkpoint, bounds, None, true, 1024, &wal_log)
            .expect("Reconstruction from checkpoint + WAL must succeed");

    // VERIFICATION:
    // 1. Entities 1..=5 must not exist (despawned)
    for id in 1u32..=5u32 {
        assert!(
            reconstructed_zone.spatial_grid.get_position(id).is_none(),
            "Entity {id} should have been despawned"
        );
    }

    // 2. Entities 6..=10 must have updated coordinates
    for id in 6u32..=10u32 {
        let expected_pos = Vec3Fix::new(
            Fixed64::from_i32((id * 20) as i32),
            Fixed64::from_i32(5),
            Fixed64::from_i32((id * 20) as i32),
        );
        assert_eq!(
            reconstructed_zone.spatial_grid.get_position(id),
            Some(expected_pos),
            "Entity {id} position mismatch after crash reconstruction"
        );
    }

    // 3. Entities 11..=50 must remain in their original positions
    for id in 11u32..=50u32 {
        let expected_pos = Vec3Fix::new(
            Fixed64::from_i32((id * 10) as i32),
            Fixed64::from_i32(0),
            Fixed64::from_i32((id * 10) as i32),
        );
        assert_eq!(
            reconstructed_zone.spatial_grid.get_position(id),
            Some(expected_pos),
            "Entity {id} baseline position mismatch"
        );
    }

    // 4. Entities 51..=55 must exist at their spawn positions
    for id in 51u32..=55u32 {
        let expected_pos = Vec3Fix::new(
            Fixed64::from_i32((id * 10) as i32),
            Fixed64::from_i32(0),
            Fixed64::from_i32((id * 10) as i32),
        );
        assert_eq!(
            reconstructed_zone.spatial_grid.get_position(id),
            Some(expected_pos),
            "Spawned entity {id} missing after reconstruction"
        );
    }

    // 5. Total entity count must be exactly 50 - 5 + 5 = 50
    assert_eq!(
        reconstructed_zone.spatial_grid.active_count(),
        50,
        "Total reconstructed entity count must be exactly 50"
    );
}
