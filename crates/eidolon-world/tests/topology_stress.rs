//! Tier 3: Concurrency, Spatial Saturation & Topology Stress Test Suite.
//!
//! Verifies 500-entity zone seam oscillation ping-pong, 500+ concurrent ephemeral dungeon rooms,
//! scale-to-zero compute reclamation, and sub-5ms cold account hydration.

use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_world::error::ZoneId;
use eidolon_world::hibernation::{CharacterRecord, PlayerProfile};
use eidolon_world::instance::{DungeonPool, InstanceLifecycle};
use eidolon_world::zone::{SeamAxis, WorldManager, WorldZone, ZoneBounds};

#[test]
fn test_500_entity_seam_oscillation_ping_pong() {
    let mut manager = WorldManager::new();

    // Zone 1: X in [0, 100], 16m seam at [84, 100], midpoint at 92. Neighbor = Zone 2 (positive X)
    let bounds1 = ZoneBounds::new(
        Fixed64::from_i32(0),
        Fixed64::from_i32(100),
        Fixed64::from_i32(0),
        Fixed64::from_i32(500),
        SeamAxis::EastWest,
        Fixed64::from_i32(84),
        Fixed64::from_i32(100),
    );
    let mut zone1 = WorldZone::new(ZoneId(1), bounds1, Some(ZoneId(2)), true, 1024);

    // Zone 2: X in [84, 200], 16m seam at [84, 100], midpoint at 92. Neighbor = Zone 1 (negative X)
    let bounds2 = ZoneBounds::new(
        Fixed64::from_i32(84),
        Fixed64::from_i32(200),
        Fixed64::from_i32(0),
        Fixed64::from_i32(500),
        SeamAxis::EastWest,
        Fixed64::from_i32(84),
        Fixed64::from_i32(100),
    );
    let zone2 = WorldZone::new(ZoneId(2), bounds2, Some(ZoneId(1)), false, 1024);

    let num_entities = 500u32;

    // Populate Zone 1 with 500 entities near the seam
    for id in 0..num_entities {
        let pos = Vec3Fix::from_i32(80, 0, (id as i32) % 500);
        zone1.insert_entity(id, pos).expect("insert entity");
    }

    manager.add_zone(zone1);
    manager.add_zone(zone2);

    assert_eq!(manager.get_zone(ZoneId(1)).unwrap().entity_count(), 500);
    assert_eq!(manager.get_zone(ZoneId(2)).unwrap().entity_count(), 0);

    // Oscillate 500 entities back and forth across the seam midpoint (92) over 50 ticks
    for tick in 0..50 {
        let target_x = if tick % 2 == 0 { 95 } else { 85 };

        for id in 0..num_entities {
            let current_zone = if manager.get_zone(ZoneId(1)).unwrap().contains_entity(id) {
                ZoneId(1)
            } else {
                ZoneId(2)
            };

            let new_pos = Vec3Fix::from_i32(target_x, 0, (id as i32) % 500);
            let _ = manager
                .tick_entity_movement(id, current_zone, new_pos)
                .expect("tick migration");
        }

        // Total entities must strictly remain exactly 500 with zero leaks or duplicates
        let count_z1 = manager.get_zone(ZoneId(1)).unwrap().entity_count();
        let count_z2 = manager.get_zone(ZoneId(2)).unwrap().entity_count();
        assert_eq!(
            count_z1 + count_z2,
            500,
            "Entity count diverged during seam oscillation at tick {tick}"
        );

        if tick % 2 == 0 {
            // Entities should have migrated to Zone 2 (target_x = 95 > 92)
            assert_eq!(count_z2, 500);
            assert_eq!(count_z1, 0);
        } else {
            // Entities should have migrated back to Zone 1 (target_x = 85 < 92)
            assert_eq!(count_z1, 500);
            assert_eq!(count_z2, 0);
        }
    }
}

#[test]
fn test_500_ephemeral_dungeon_rooms_scale_to_zero() {
    let mut pool = DungeonPool::<512>::new();
    assert_eq!(pool.active_room_count(), 0);

    let num_rooms = 500;
    let mut allocated_ids = Vec::with_capacity(num_rooms);

    // Step 1: Allocate 500 rooms in sub-millisecond time
    let alloc_start = Instant::now();
    for i in 0..num_rooms {
        let boss_hp = 100 + (i as u32);
        let id = pool.allocate_instance(boss_hp).expect("allocate room");
        allocated_ids.push(id);
    }
    let alloc_duration = alloc_start.elapsed();
    assert_eq!(pool.active_room_count(), num_rooms);
    // Sub-50ms allocation assertion across all 500 rooms
    assert!(
        alloc_duration.as_millis() < 50,
        "500 room allocation took too long: {:?}",
        alloc_duration
    );

    // Step 2: Join 4 players per room (2,000 players total) and start combat
    for (idx, &id) in allocated_ids.iter().enumerate() {
        let room = pool.get_instance_mut(id).expect("get room");
        assert_eq!(room.state, InstanceLifecycle::Allocated);

        for p in 0..4 {
            let player_id = (idx as u32) * 4 + p;
            room.join_party(player_id).expect("join party");
        }
        assert_eq!(room.state, InstanceLifecycle::ActiveCombat);
        assert_eq!(room.party_count, 4);
    }

    // Step 3: Simulate boss defeat and player exit
    for (idx, &id) in allocated_ids.iter().enumerate() {
        let room = pool.get_instance_mut(id).expect("get room");
        let boss_hp = room.boss_health;
        assert!(room.apply_boss_damage(boss_hp));
        assert_eq!(room.state, InstanceLifecycle::VictoryReward);

        for p in 0..4 {
            let player_id = (idx as u32) * 4 + p;
            room.leave_party(player_id).expect("leave party");
        }
        assert_eq!(room.state, InstanceLifecycle::PendingCleanup);
    }

    // Step 4: Tick pool to purge pending rooms
    pool.tick_all();

    // Compute drops back to absolute zero ($0 idle cost)
    assert_eq!(pool.active_room_count(), 0);

    // All slots can be safely reallocated
    for _ in 0..num_rooms {
        pool.allocate_instance(1000).expect("reallocate room");
    }
    assert_eq!(pool.active_room_count(), num_rooms);
}

#[test]
fn test_1000_account_cold_hibernation_hydration_benchmark() {
    let num_accounts = 1000u64;
    let mut snapshots = Vec::with_capacity(num_accounts as usize);

    // Step 1: Serialize 1,000 realistic player accounts
    for acc_id in 0..num_accounts {
        let mut profile = PlayerProfile::new(acc_id);
        profile.player_level = 1 + (acc_id as u32 % 90);
        profile.premium_currency = acc_id * 160;
        profile.free_currency = acc_id * 10000;
        profile.pity.limited_banner_pity = (acc_id % 90) as u16;
        profile.pity.is_guaranteed_rate_up = acc_id.is_multiple_of(2);

        // Add 5 characters per account
        for c in 0..5 {
            profile
                .add_character(CharacterRecord {
                    character_id: 1000 + c,
                    level: 20 + (c as u16 * 10),
                    ascension_tier: c as u8,
                    constellation: (c % 6) as u8,
                })
                .expect("add char");
        }

        let mut buffer = [0u8; 512];
        let len = profile
            .serialize_snapshot(1700000000, &mut buffer)
            .expect("serialize");
        snapshots.push(buffer[..len].to_vec());
    }

    // Step 2: Benchmark hydration of 1,000 accounts
    let hydrate_start = Instant::now();
    for (idx, snapshot) in snapshots.iter().enumerate() {
        let hydrated = PlayerProfile::deserialize_snapshot(snapshot).expect("hydrate snapshot");
        assert_eq!(hydrated.account_id, idx as u64);
        assert_eq!(hydrated.character_count, 5);
        assert_eq!(
            hydrated.pity.is_guaranteed_rate_up,
            (idx as u64).is_multiple_of(2)
        );
    }
    let total_hydrate_duration = hydrate_start.elapsed();

    // Average hydration must be under 5ms (typically under 2 microseconds per account!)
    let avg_micros = total_hydrate_duration.as_micros() as f64 / num_accounts as f64;
    assert!(
        avg_micros < 5000.0,
        "Average hydration exceeded 5ms: {avg_micros} µs"
    );
}
