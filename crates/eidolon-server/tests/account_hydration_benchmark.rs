//! Milestone 9.5: Cold Account Hibernation to Hot Zone Hydration Lifecycle Benchmark.
//!
//! Benchmarks sub-5ms hydration of cold account snapshots into active zone spatial grids.
//! Asserts snapshot size <256 bytes and verifies checksum corruption rejection.

use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_world::hibernation::{hydrate_player_into_zone, CharacterRecord, PlayerProfile};
use eidolon_world::zone::{SeamAxis, WorldZone, ZoneBounds};
use eidolon_world::{WorldError, ZoneId};

#[test]
fn test_milestone_9_5_account_hydration_performance_and_integrity() {
    let bounds = ZoneBounds::new(
        Fixed64::from_i32(0),
        Fixed64::from_i32(5000),
        Fixed64::from_i32(0),
        Fixed64::from_i32(5000),
        SeamAxis::EastWest,
        Fixed64::from_i32(4984),
        Fixed64::from_i32(5000),
    );
    let mut zone = WorldZone::new(ZoneId(1), bounds, None, true, 2048);

    // 1. Create a cold player snapshot with full roster and pity data
    let mut profile = PlayerProfile::new(88888888);
    profile.player_level = 80;
    profile.premium_currency = 32_000;
    profile.free_currency = 12_500_000;
    profile.pity.limited_banner_pity = 70;
    profile.pity.is_guaranteed_rate_up = true;

    profile
        .add_character(CharacterRecord {
            character_id: 101,
            level: 90,
            ascension_tier: 6,
            constellation: 4,
        })
        .unwrap();
    profile
        .add_character(CharacterRecord {
            character_id: 202,
            level: 85,
            ascension_tier: 6,
            constellation: 1,
        })
        .unwrap();

    let mut snapshot_buffer = [0u8; 512];
    let snapshot_len = profile
        .serialize_snapshot(1726000000, &mut snapshot_buffer)
        .expect("Snapshot serialization must succeed");

    // Invariant: Snapshot size must be compact (<256 bytes)
    assert!(
        snapshot_len < 256,
        "Snapshot size {snapshot_len} exceeded 256 bytes limit"
    );

    // 2. Benchmark hydration pipeline for 1,000 accounts
    let total_accounts = 1_000;
    let start = Instant::now();

    for i in 0..total_accounts {
        let spawn_pos = Vec3Fix::new(
            Fixed64::from_i32((i % 100) * 10),
            Fixed64::from_i32(0),
            Fixed64::from_i32((i / 100) * 10),
        );
        let entity_id = 1000 + i as u32;

        let account_state = hydrate_player_into_zone(&profile, &mut zone, spawn_pos, entity_id)
            .expect("Player hydration must succeed");

        assert_eq!(account_state.account_id, 88888888);
        assert_eq!(account_state.premium_currency, 32_000);
    }

    let elapsed = start.elapsed();
    let per_account_us = elapsed.as_micros() as f64 / total_accounts as f64;

    println!(
        "[Hydration Benchmark] 1,000 accounts hydrated in {:?} ({:.2} µs/account)",
        elapsed, per_account_us
    );

    // Invariant: Sub-5ms (<5,000 µs) per-account hydration target
    assert!(
        per_account_us < 5_000.0,
        "Hydration took {:.2} µs, exceeding 5ms budget",
        per_account_us
    );

    // 3. Verify Checksum Corruption Rejection
    let mut corrupted_snapshot = [0u8; 512];
    corrupted_snapshot[..snapshot_len].copy_from_slice(&snapshot_buffer[..snapshot_len]);
    corrupted_snapshot[36] ^= 0x55; // Corrupt payload byte

    let corrupt_res = PlayerProfile::deserialize_snapshot(&corrupted_snapshot[..snapshot_len]);
    assert_eq!(
        corrupt_res.unwrap_err(),
        WorldError::SnapshotCorrupted,
        "Corrupted snapshot must be rejected"
    );
}
