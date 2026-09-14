//! Exhaustive integration test suite for Phase 21: Autonomous NPC Ecosystem.
//!
//! Validates sensory perception (sight cones, acoustic detection, stealth ratings),
//! threat aggregation with taunts and hysteresis, flocking separation steering,
//! zero-allocation behavior tree evaluation, leashing resets, and 2,000-mob LOD scaling.

use eidolon_core::{evaluate_perception, BehaviorTree, BtNodeType, BtStatus, SensoryProfile};
use eidolon_world::{
    compute_separation, NpcEcosystemManager, NpcEntity, NpcLodTier, NpcState, ThreatTable,
};

#[test]
fn test_sensory_perception_fov_and_hearing() {
    let sensory = SensoryProfile::new(20.0, 90.0, 8.0, 10);

    let npc_pos = (0.0, 0.0, 0.0);
    let npc_yaw = 0.0; // Facing +Z: forward vector is (0, 0, 1)

    // Case 1: Player directly in front at 10m (within 90 deg cone)
    let p_front = (0.0, 0.0, 10.0);
    let res_front = evaluate_perception(npc_pos, npc_yaw, p_front, 0, &sensory);
    assert!(res_front.perceived);
    assert!(res_front.is_visual);

    // Case 2: Player directly behind at 15m (outside FOV cone, outside hearing range)
    let p_behind_far = (0.0, 0.0, -15.0);
    let res_behind_far = evaluate_perception(npc_pos, npc_yaw, p_behind_far, 0, &sensory);
    assert!(!res_behind_far.perceived);
    assert!(!res_behind_far.is_visual);
    assert!(!res_behind_far.is_acoustic);

    // Case 3: Player directly behind at 5m (outside FOV cone, but within hearing range 8m)
    let p_behind_near = (0.0, 0.0, -5.0);
    let res_behind_near = evaluate_perception(npc_pos, npc_yaw, p_behind_near, 0, &sensory);
    assert!(res_behind_near.perceived);
    assert!(!res_behind_near.is_visual);
    assert!(res_behind_near.is_acoustic);

    // Case 4: Stealth detection: player stealth > NPC stealth detection rating
    let res_stealth = evaluate_perception(npc_pos, npc_yaw, p_front, 50, &sensory);
    assert!(!res_stealth.perceived);
    assert!(!res_stealth.is_visual);
}

#[test]
fn test_threat_table_aggregation_and_hysteresis() {
    let mut table = ThreatTable::new();
    let tank_id = 1001;
    let dps_id = 2002;

    // Tank generates 1,000 threat
    table.add_threat(tank_id, 1000, true, 10);
    let target = table.select_target(None, false, 10);
    assert_eq!(target, Some(tank_id));

    // DPS deals 1,050 threat (105% of tank)
    // For melee (110% hysteresis threshold), tank must still retain aggro!
    table.add_threat(dps_id, 1050, false, 11);
    let target_melee = table.select_target(Some(tank_id), true, 11);
    assert_eq!(
        target_melee,
        Some(tank_id),
        "Tank should hold aggro below 110% threshold"
    );

    // For ranged (130% hysteresis threshold), tank retains aggro even longer
    let target_ranged = table.select_target(Some(tank_id), false, 11);
    assert_eq!(
        target_ranged,
        Some(tank_id),
        "Tank should hold aggro below 130% threshold"
    );

    // DPS pulls threat past 110% (1,150 threat)
    table.add_threat(dps_id, 100, false, 12);
    let target_swapped = table.select_target(Some(tank_id), true, 12);
    assert_eq!(
        target_swapped,
        Some(dps_id),
        "DPS should pull aggro above 110% threshold in melee"
    );

    // But for ranged (130%), 1,150 threat is still below 1,300 threshold
    let target_still_tank = table.select_target(Some(tank_id), false, 12);
    assert_eq!(
        target_still_tank,
        Some(tank_id),
        "Tank should hold aggro against ranged competitor below 130%"
    );

    // Tank taunts! Locks aggro for 60 ticks
    table.apply_taunt(tank_id, 60, 15);
    let target_taunted = table.select_target(Some(dps_id), true, 16);
    assert_eq!(
        target_taunted,
        Some(tank_id),
        "Taunt must override threat table"
    );

    // After taunt expires at tick 75
    let target_post_taunt = table.select_target(Some(tank_id), true, 76);
    assert_eq!(
        target_post_taunt,
        Some(dps_id),
        "Aggro reverts to highest threat after taunt expires"
    );
}

#[test]
fn test_flocking_separation_steering() {
    let center = (10.0, 10.0); // (X, Z)
    let neighbors = [
        (10.5, 10.0), // 0.5m east (+X)
        (10.0, 10.5), // 0.5m north (+Z)
        (9.5, 10.0),  // 0.5m west (-X)
        (25.0, 25.0), // 21m away (beyond 2.0m radius)
    ];

    let sep = compute_separation(center, &neighbors, 2.0);
    // Neighbors are symmetric in east and west, but there is one to the north (+Z).
    // The separation vector must push away from north, i.e. negative Z!
    assert!(
        sep.1 < 0.0,
        "Separation vector must push away from crowded neighbor: {sep:?}"
    );
    assert!(
        sep.0.abs() < 0.001,
        "Symmetric east-west neighbors cancel X steering: {sep:?}"
    );
}

#[test]
fn test_behavior_tree_evaluation() {
    let mut bt = BehaviorTree::new();

    // Build: Sequence [ Condition, Action ]
    // Sequence is at index 0, pointing to first child at index 1 with count 2
    let seq_id = bt.add_node(BtNodeType::Sequence(1, 2));
    assert_eq!(seq_id, Some(0));

    let cond_id = bt.add_node(BtNodeType::Condition(1)); // Health < 20%
    assert_eq!(cond_id, Some(1));

    let action_id = bt.add_node(BtNodeType::Action(10)); // Drink potion
    assert_eq!(action_id, Some(2));

    bt.set_root(0);

    let status = bt.tick(
        |act| {
            if act == 10 {
                BtStatus::Success
            } else {
                BtStatus::Failure
            }
        },
        |cond| cond == 1,
    );

    assert_eq!(status, BtStatus::Success);
}

#[test]
fn test_npc_leashing_and_evade_reset() {
    let mut manager = NpcEcosystemManager::new();
    let spawn = (100.0, 0.0, 100.0);
    let sensory = SensoryProfile::new(20.0, 90.0, 10.0, 0);

    let npc = NpcEntity::new(1, 101, spawn, 500, 20.0, sensory);
    manager.add_npc(npc);

    // Mob takes damage, enters combat
    manager.apply_damage(1, 999, 100, 1);
    let mob = manager.get_npc(1).unwrap();
    assert_eq!(mob.state, NpcState::Chasing);
    assert_eq!(mob.health, 400);
    assert_eq!(mob.lod_tier, NpcLodTier::ActiveCombat);

    // Move mob past leash radius (spawn is 100, leash is 20)
    if let Some(m) = manager.get_npc_mut(1) {
        m.position = (130.0, 0.0, 100.0); // 30m away > 20m leash
    }

    // Tick simulation: mob should detect leash violation and start Evading
    let players = [(999, (125.0, 0.0, 100.0), 0)];
    manager.tick(2, 0.050, &players);

    let mob = manager.get_npc(1).unwrap();
    assert_eq!(mob.state, NpcState::Evading);
    assert!(mob.current_target.is_none(), "Evade clears target");

    // Simulate multiple ticks until mob paths all the way back to spawn
    for t in 3..100 {
        manager.tick(t, 0.050, &players);
        if manager.get_npc(1).unwrap().state == NpcState::Idle {
            break;
        }
    }

    let mob_reset = manager.get_npc(1).unwrap();
    assert_eq!(mob_reset.state, NpcState::Idle);
    assert_eq!(
        mob_reset.health, mob_reset.max_health,
        "Health resets to 100% after evade"
    );
    assert!((mob_reset.position.0 - 100.0).abs() < 1.0);
    assert!((mob_reset.position.2 - 100.0).abs() < 1.0);
}

#[test]
fn test_high_density_2000_mob_lod_scaling() {
    let mut manager = NpcEcosystemManager::new();
    let sensory = SensoryProfile::new(20.0, 90.0, 10.0, 0);

    // Spawn 2,000 mobs across a 1,000m x 1,000m world
    for i in 0..2000 {
        let x = (i % 50) as f32 * 20.0;
        let z = (i / 50) as f32 * 20.0;
        let npc = NpcEntity::new(i as u64, 1, (x, 0.0, z), 200, 25.0, sensory);
        manager.add_npc(npc);
    }
    assert_eq!(manager.len(), 2000);

    // Place a player at (100.0, 0.0, 100.0)
    let players = [(100.0f32, 0.0f32, 100.0f32)];
    manager.update_lod_tiers(&players);

    let mut active_count = 0;
    let mut mid_count = 0;
    let mut dormant_count = 0;

    for npc in manager.npcs() {
        match npc.lod_tier {
            NpcLodTier::ActiveCombat => active_count += 1,
            NpcLodTier::MidTier => mid_count += 1,
            NpcLodTier::Dormant => dormant_count += 1,
        }
    }

    // Mobs near (100, 0, 100) within 15m are ActiveCombat
    assert!(
        active_count > 0,
        "Expected active combat tier mobs near player"
    );
    // Mobs between 15m and 50m are MidTier
    assert!(mid_count > 0, "Expected mid tier mobs in proximity");
    // Majority of 2,000 mobs in wilderness are Dormant (0 CPU cost)
    assert!(
        dormant_count > 1900,
        "Expected overwhelming majority of mobs to be Dormant: {dormant_count}"
    );

    // Simulate 20 ticks (1 full second at 20 Hz)
    let player_inputs = [(1u64, (100.0, 0.0, 100.0), 0u32)];
    for tick in 1..=20 {
        manager.tick(tick, 0.050, &player_inputs);
    }
}
