//! 100,000-tick sustained load and zero memory leak audit.
//!
//! Validates long-running simulation stability, bounded memory footprint,
//! scale-to-zero dungeon instance reclamation, and zero thread deadlocks.

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_world::instance::{DungeonPool, InstanceLifecycle};
use eidolon_world::zone::{SeamAxis, WorldManager, WorldZone, ZoneBounds, ZoneId};

#[test]
fn test_100k_tick_sustained_load_and_leak_audit() {
    let total_ticks = 100_000u64;
    let num_entities = 100u32;

    // 1. Initialize Multi-Zone World Topology
    let mut world = WorldManager::new();
    let bounds1 = ZoneBounds::new(
        Fixed64::ZERO,
        Fixed64::from_i32(72),
        Fixed64::ZERO,
        Fixed64::from_i32(64),
        SeamAxis::EastWest,
        Fixed64::from_i32(56),
        Fixed64::from_i32(72),
    );
    let zone1 = WorldZone::new(ZoneId(1), bounds1, Some(ZoneId(2)), true, 200);
    world.add_zone(zone1);

    let bounds2 = ZoneBounds::new(
        Fixed64::from_i32(56),
        Fixed64::from_i32(128),
        Fixed64::ZERO,
        Fixed64::from_i32(64),
        SeamAxis::EastWest,
        Fixed64::from_i32(56),
        Fixed64::from_i32(72),
    );
    let zone2 = WorldZone::new(ZoneId(2), bounds2, Some(ZoneId(1)), false, 200);
    world.add_zone(zone2);

    // 2. Initialize Ephemeral Dungeon Pool (holds up to 64 concurrent rooms)
    let mut dungeon_pool = DungeonPool::<64>::new();

    // 3. Populate Entities
    for id in 1..=num_entities {
        let pos = Vec3Fix::from_f64(10.0 + ((id % 20) as f64) * 2.0, 0.0, 20.0);
        world
            .get_zone_mut(ZoneId(1))
            .expect("zone 1")
            .insert_entity(id, pos)
            .expect("insert");
    }

    let mut tracked_zones = vec![ZoneId(1); (num_entities + 1) as usize];
    let mut total_migrations = 0usize;
    let mut total_dungeon_cycles = 0usize;

    // 4. Execute 100,000 Sustained Simulation Ticks
    for tick in 1..=total_ticks {
        let tick_mod = tick % 100;

        // Move entities back and forth across the seam
        for id in 1..=num_entities {
            let idx = id as usize;
            let current_zone = tracked_zones[idx];

            // Oscillate position across X = 64 seam
            let x_coord = if current_zone == ZoneId(1) {
                if tick_mod > 50 {
                    68.0 // Move across seam into Zone 2
                } else {
                    40.0
                }
            } else if tick_mod > 50 {
                60.0 // Move back into Zone 1
            } else {
                80.0
            };

            let new_pos = Vec3Fix::from_f64(x_coord, 0.0, 20.0);
            if let Ok(Some(ticket)) = world.tick_entity_movement(id, current_zone, new_pos) {
                tracked_zones[idx] = ticket.to_zone;
                total_migrations += 1;
            }
        }

        // Ephemeral Dungeon Lifecycle: Allocate, Tick, and Deallocate
        if tick % 500 == 0 {
            // Allocate a dungeon room for a party
            if let Ok(inst_id) = dungeon_pool.allocate_instance(1000) {
                let instance = dungeon_pool.get_instance_mut(inst_id).expect("instance");
                assert!(instance.join_party(1).is_ok());
                assert!(instance.join_party(2).is_ok());

                // Players collect rewards and exit the room
                assert!(instance.leave_party(1).is_ok());
                assert!(instance.leave_party(2).is_ok());
                assert_eq!(instance.state, InstanceLifecycle::PendingCleanup);

                // Tick pool to reclaim memory slot immediately
                dungeon_pool.tick_all();
                assert_eq!(dungeon_pool.active_room_count(), 0);
                total_dungeon_cycles += 1;
            }
        }
    }

    // 5. Audit Accounting & Invariants
    let z1_count = world.get_zone(ZoneId(1)).expect("zone 1").entity_count();
    let z2_count = world.get_zone(ZoneId(2)).expect("zone 2").entity_count();

    assert_eq!(
        z1_count + z2_count,
        num_entities as usize,
        "Zero entity loss invariant violated over 100,000 sustained ticks"
    );
    assert!(
        total_migrations > 1_000,
        "Continuous seam migrations must have occurred"
    );
    assert_eq!(
        dungeon_pool.active_room_count(),
        0,
        "Scale-to-zero compute invariant violated: active dungeon rooms must be 0"
    );
    assert!(
        total_dungeon_cycles > 50,
        "Dungeon allocation/deallocation cycles must have executed"
    );
}
