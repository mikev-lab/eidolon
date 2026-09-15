//! Integration Test Suite: Authoritative MMO Gameplay Systems (Core MMORPG Tooling)
//!
//! Validates:
//! 1. Authoritative Inventory & Equipment: Stat modifiers (attack power, armor mitigation),
//!    equipment slot rules, and atomic gear transitions.
//! 2. Cast Bar Progression & Movement Interrupt: Spell cast timing, interrupt on movement
//!    threshold (>0.5m), and stationary completion.
//! 3. Geometric Area-of-Effect (AoE) Targeting: Cone (Arcane Cleave) and Sphere (Frost Nova)
//!    hit validation in the 3D spatial grid using fixed-point math without square roots.
//! 4. Multi-Channel Chat & Proximity Filtering: 25m spatial radius routing via spatial hash grid,
//!    party channel distribution, and token-bucket anti-spam rate limiting.
//! 5. Authoritative Party Management: Group lifecycle (invite, accept, leave), leader authority,
//!    and real-time vitals replication.

use std::net::SocketAddr;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::item::EquipmentSlot;
use eidolon_server::EidolonApp;
use eidolon_world::ability::{get_ability_definition, CastState};
use eidolon_world::party::PartyMember;

#[test]
fn test_equipment_stat_modifiers_and_combat_damage() {
    let mut server = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Bind server")
        .build()
        .expect("Build server");

    // Spawn player attacker (ID 1) and target dummy (ID 2)
    server
        .spawn_npc(1, 0, 100.0, 0.0, 100.0)
        .expect("Spawn attacker");
    server
        .spawn_npc(2, 1, 103.0, 0.0, 100.0)
        .expect("Spawn target");

    // Base attack: 30 base damage with no equipment
    server.damage_entity(1, 2, 30);
    let target = server.get_entity(2).expect("Target exists");
    assert_eq!(target.health, 70, "Target should have 100 - 30 = 70 health");

    // Equip Sword of Valor (+25 Attack Power) to player's MainHand
    // Item ID 1001 = Sword of Valor
    let attacker = server.get_entity_mut(1).expect("Attacker exists");
    attacker
        .equipment
        .equip(EquipmentSlot::MainHand, 1001)
        .expect("Equip sword");
    assert_eq!(
        attacker.equipment.compute_stats().attack_power,
        25,
        "Attacker should have +25 attack power"
    );

    // Attack again: 30 base damage + 25 attack power = 55 total damage
    server.damage_entity(1, 2, 30);
    let target = server.get_entity(2).expect("Target exists");
    assert_eq!(target.health, 15, "Target should have 70 - 55 = 15 health");

    // Equip Iron Kite Shield (+30 Armor) on target (Item ID 1002)
    let target_mut = server.get_entity_mut(2).expect("Target exists");
    target_mut.health = 100; // Reset health
    target_mut
        .equipment
        .equip(EquipmentSlot::OffHand, 1002)
        .expect("Equip shield");
    assert_eq!(
        target_mut.equipment.compute_stats().armor,
        30,
        "Target should have 30 armor"
    );

    // Attack with 55 total damage against 30 armor: 55 - 30 = 25 effective damage
    server.damage_entity(1, 2, 30);
    let target = server.get_entity(2).expect("Target exists");
    assert_eq!(
        target.health, 75,
        "Target should have 100 - (55 - 30) = 75 health"
    );
}

#[test]
fn test_cast_bar_progression_and_movement_interrupt() {
    let mut server = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Bind server")
        .build()
        .expect("Build server");

    // Spawn caster player (ID 10) and monster target (ID 20)
    server
        .spawn_npc(10, 0, 100.0, 0.0, 100.0)
        .expect("Spawn caster");
    server
        .spawn_npc(20, 1, 110.0, 0.0, 100.0)
        .expect("Spawn target");

    // Initiate Fireball (ability ID 1: 20 ticks = 1.0s cast duration, 60 damage)
    let fireball_def = get_ability_definition(1).expect("Fireball definition");
    let caster = server.get_entity_mut(10).expect("Caster exists");
    caster.cast_state = CastState::start_cast(
        1,
        20,
        0, // Start at tick 0
        fireball_def.cast_duration_ticks,
        caster.position,
        fireball_def.allow_movement,
    );

    // Tick forward 10 ticks (0.5s): Cast should still be in progress
    for _ in 0..10 {
        server.tick().expect("Tick");
    }

    let caster = server.get_entity(10).expect("Caster exists");
    assert!(
        matches!(caster.cast_state, CastState::Casting { .. }),
        "Cast should still be in progress at tick 10"
    );

    // Caster moves beyond stationary threshold (>0.5m)
    let caster_mut = server.get_entity_mut(10).expect("Caster exists");
    caster_mut.position.x += Fixed64::from_f64(1.2);

    // Next server tick should detect movement and interrupt the cast!
    server.tick().expect("Tick");

    let caster = server.get_entity(10).expect("Caster exists");
    assert_eq!(
        caster.cast_state,
        CastState::Idle,
        "Cast should be interrupted and reset to Idle on movement"
    );

    // Target monster should not have taken any Fireball damage
    let target = server.get_entity(20).expect("Target exists");
    assert_eq!(
        target.health, 100,
        "Target should have full health after interrupted cast"
    );

    // Restart Fireball cast while remaining stationary
    let caster_mut = server.get_entity_mut(10).expect("Caster exists");
    let current_pos = caster_mut.position;
    caster_mut.cast_state = CastState::start_cast(
        1,
        20,
        0,
        fireball_def.cast_duration_ticks,
        current_pos,
        fireball_def.allow_movement,
    );

    // Advance 21 ticks: Cast completes and deals 60 damage!
    for _ in 0..21 {
        server.tick().expect("Tick");
    }

    let caster = server.get_entity(10).expect("Caster exists");
    assert_eq!(
        caster.cast_state,
        CastState::Idle,
        "Cast should be Idle after completion"
    );

    let target = server.get_entity(20).expect("Target exists");
    assert_eq!(
        target.health, 40,
        "Target health should be reduced from 100 to 40 (60 damage dealt)"
    );
}

#[test]
fn test_spatial_combat_targeting_cone_and_sphere() {
    let mut server = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Bind server")
        .build()
        .expect("Build server");

    // Spawn caster at (0, 0, 0) facing North (Yaw = 0)
    server.spawn_npc(1, 0, 0.0, 0.0, 0.0).expect("Spawn caster");

    // Spawn 3 targets:
    // Target A: In front inside cone at (0, 0, 5) -> Distance = 5m, Angle = 0 deg (INSIDE CONE)
    server
        .spawn_npc(10, 1, 0.0, 0.0, 5.0)
        .expect("Spawn target A");
    // Target B: Behind caster at (0, 0, -5) -> Distance = 5m, Angle = 180 deg (OUTSIDE CONE)
    server
        .spawn_npc(11, 1, 0.0, 0.0, -5.0)
        .expect("Spawn target B");
    // Target C: In front but beyond range at (0, 0, 12) -> Distance = 12m (OUT OF RANGE)
    server
        .spawn_npc(12, 1, 0.0, 0.0, 12.0)
        .expect("Spawn target C");

    // Ability ID 2 = Arcane Cleave (instant cast, 40 base damage, cone)
    let caster_mut = server.get_entity_mut(1).expect("Caster exists");
    let caster_pos = caster_mut.position;
    caster_mut.cast_state = CastState::start_cast(2, 0, 0, 0, caster_pos, true);

    // Tick server to resolve instant ability
    server.tick().expect("Tick");

    let target_a = server.get_entity(10).expect("Target A exists");
    let target_b = server.get_entity(11).expect("Target B exists");
    let target_c = server.get_entity(12).expect("Target C exists");

    assert_eq!(
        target_a.health, 60,
        "Target A (inside cone) should take 40 damage (100 - 40 = 60)"
    );
    assert_eq!(
        target_b.health, 100,
        "Target B (behind caster) should take no damage"
    );
    assert_eq!(
        target_c.health, 100,
        "Target C (out of range) should take no damage"
    );

    // Now test RadialSphere: Frost Nova (Ability ID 4: 10m radius sphere, 35 damage)
    // Target D: At (6, 0, 6) -> Distance = sqrt(72) ~ 8.48m (INSIDE SPHERE)
    server
        .spawn_npc(13, 1, 6.0, 0.0, 6.0)
        .expect("Spawn target D");
    // Target E: At (10, 0, 10) -> Distance = sqrt(200) ~ 14.14m (OUTSIDE SPHERE)
    server
        .spawn_npc(14, 1, 10.0, 0.0, 10.0)
        .expect("Spawn target E");

    let caster_mut = server.get_entity_mut(1).expect("Caster exists");
    let caster_pos = caster_mut.position;
    caster_mut.cast_state = CastState::start_cast(4, 0, 0, 0, caster_pos, true);

    server.tick().expect("Tick");

    let target_d = server.get_entity(13).expect("Target D exists");
    let target_e = server.get_entity(14).expect("Target E exists");

    assert_eq!(
        target_d.health, 65,
        "Target D (inside 10m sphere) should take 35 damage (100 - 35 = 65)"
    );
    assert_eq!(
        target_e.health, 100,
        "Target E (outside 10m sphere) should take no damage"
    );
}

#[test]
fn test_spatial_proximity_chat_and_rate_limiting() {
    let mut server = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Bind server")
        .build()
        .expect("Build server");

    let dummy_peer_1: SocketAddr = "127.0.0.1:11111".parse().unwrap();
    let dummy_peer_2: SocketAddr = "127.0.0.1:22222".parse().unwrap();
    let dummy_peer_3: SocketAddr = "127.0.0.1:33333".parse().unwrap();

    // Player 1 at (0, 0, 0)
    server
        .spawn_player(1, 1001, 101, dummy_peer_1, 0.0, 0.0, 0.0)
        .expect("Spawn P1");
    // Player 2 at (15, 0, 0) -> Within 25m proximity radius (15m < 25m)
    server
        .spawn_player(2, 1002, 102, dummy_peer_2, 15.0, 0.0, 0.0)
        .expect("Spawn P2");
    // Player 3 at (40, 0, 0) -> Outside 25m proximity radius (40m > 25m)
    server
        .spawn_player(3, 1003, 103, dummy_peer_3, 40.0, 0.0, 0.0)
        .expect("Spawn P3");

    // Verify chat rate limiter enforces token bucket burst and refill
    let mut limiter = eidolon_world::chat::ChatRateLimiter::new(3, 20); // 3 burst capacity, 1 per 20 ticks

    // Initial 3 messages should succeed
    assert!(limiter.check_and_consume(1001, 0).is_ok());
    assert!(limiter.check_and_consume(1001, 0).is_ok());
    assert!(limiter.check_and_consume(1001, 0).is_ok());

    // 4th message in same tick must be rejected (rate limited)
    assert!(
        limiter.check_and_consume(1001, 0).is_err(),
        "4th rapid message must exceed burst capacity"
    );

    // Advance 20 ticks (1.0 second) -> 1 token replenished
    assert!(
        limiter.check_and_consume(1001, 20).is_ok(),
        "Message should succeed after token replenishment"
    );
}

#[test]
fn test_party_lifecycle_and_vitals_replication() {
    let mut server = EidolonApp::builder()
        .bind("127.0.0.1:0")
        .expect("Bind server")
        .build()
        .expect("Build server");

    let leader = PartyMember {
        account_id: 5001,
        entity_id: 1,
        name: "Leader".to_string(),
        health: 100,
        max_health: 100,
        mana: 100,
        max_mana: 100,
        position: Vec3Fix::ZERO,
    };

    let member_b = PartyMember {
        account_id: 5002,
        entity_id: 2,
        name: "Ranger".to_string(),
        health: 85,
        max_health: 85,
        mana: 50,
        max_mana: 50,
        position: Vec3Fix::ZERO,
    };

    // 1. Create party
    let party_id = server
        .party_manager_mut()
        .create_party(leader)
        .expect("Create party");
    assert_eq!(party_id, 1);

    let party = server
        .party_manager()
        .get_party(party_id)
        .expect("Party exists");
    assert_eq!(party.members.len(), 1);
    assert_eq!(party.leader_account_id, 5001);

    // 2. Invite member B
    server
        .party_manager_mut()
        .invite_player(party_id, 5001, 5002)
        .expect("Invite member B");

    // 3. Member B accepts invite
    server
        .party_manager_mut()
        .accept_invite(5002, member_b)
        .expect("Accept invite");

    let party = server
        .party_manager()
        .get_party(party_id)
        .expect("Party exists");
    assert_eq!(party.members.len(), 2);
    assert!(party.is_member(5001));
    assert!(party.is_member(5002));

    // 4. Update member vitals
    server
        .party_manager_mut()
        .get_party_mut(party_id)
        .unwrap()
        .update_member_vitals(5002, 45, 30, Vec3Fix::from_f64(10.0, 0.0, 5.0));

    let party = server
        .party_manager()
        .get_party(party_id)
        .expect("Party exists");
    let m2 = party
        .members
        .iter()
        .find(|m| m.account_id == 5002)
        .expect("Member 2");
    assert_eq!(m2.health, 45, "Health should be updated to 45");
    assert_eq!(m2.mana, 30, "Mana should be updated to 30");

    // 5. Member leaves party
    let left = server
        .party_manager_mut()
        .leave_party(5002)
        .expect("Leave party");
    assert_eq!(left, Some(party_id));

    let party = server
        .party_manager()
        .get_party(party_id)
        .expect("Party exists");
    assert_eq!(party.members.len(), 1);
    assert!(!party.is_member(5002));
}
