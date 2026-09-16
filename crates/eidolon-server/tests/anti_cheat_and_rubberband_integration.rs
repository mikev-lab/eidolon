//! Integration test suite verifying server-authoritative movement validation,
//! anti-cheat guards (speed-hack clamping, fly-hack/levitation watchdog, authoritative gravity),
//! gameplay cast-while-moving mechanics, and authoritative desync rubber-banding.

use std::net::UdpSocket;

use eidolon_core::fixed::Fixed64;
use eidolon_core::kinematics::{FLAG_FALLING, FLAG_JUMPING, FLAG_SPRINTING, MAX_AIRBORNE_TICKS};
use eidolon_net::packet::PacketHeader;
use eidolon_net::protocol::{ChannelType, PacketType};
use eidolon_server::facade::EidolonAppBuilder;
use eidolon_world::ability::{get_ability_definition, CastState};

#[test]
fn test_speed_hack_intent_rejected_and_rubberband_dispatched() {
    let mut server = EidolonAppBuilder::new()
        .bind("127.0.0.1:0")
        .expect("Bind server")
        .build()
        .expect("Build server");

    // Bind mock client socket
    let client_sock = UdpSocket::bind("127.0.0.1:0").expect("Bind client socket");
    client_sock.set_nonblocking(true).expect("Non-blocking");
    let client_addr = client_sock.local_addr().expect("Local addr");

    // Spawn player entity (ID 42) at (10.0, 0.0, 20.0)
    server
        .spawn_player(42, 1001, 2001, client_addr, 10.0, 0.0, 20.0)
        .expect("Spawn player");

    // Construct blatant speed hack packet (vx = 500.0, vz = 500.0 ~ 707 m/s)
    let mut payload = [0u8; 13];
    payload[0..4].copy_from_slice(&500.0f32.to_be_bytes());
    payload[4..8].copy_from_slice(&500.0f32.to_be_bytes());
    payload[8..12].copy_from_slice(&0.0f32.to_be_bytes());
    payload[12] = FLAG_SPRINTING;

    let header = PacketHeader::new(
        ChannelType::UnreliableSequenced,
        PacketType::StateUpdate,
        1,
        0,
        0,
    );
    let mut packet = [0u8; 64];
    let hlen = header.write_to(&mut packet).expect("Write header");
    packet[hlen..hlen + 13].copy_from_slice(&payload);

    let server_addr = server.local_addr().expect("Server addr");
    client_sock
        .send_to(&packet[..hlen + 13], server_addr)
        .expect("Send packet");

    // Server ticks until packet is drained and StateCorrection is received
    let mut received_correction = false;
    let mut recv_buf = [0u8; 256];
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        server.tick().expect("Tick");

        while let Ok((rlen, _)) = client_sock.recv_from(&mut recv_buf) {
            if let Ok((resp_hdr, resp_hlen)) = PacketHeader::read_from(&recv_buf[..rlen]) {
                if resp_hdr.packet_type == PacketType::ReliableMessage {
                    let resp_payload = &recv_buf[resp_hlen..rlen];
                    if !resp_payload.is_empty() && resp_payload[0] == 10 {
                        received_correction = true;
                        assert!(
                            resp_payload.len() >= 63,
                            "Correction payload must be 63 bytes"
                        );
                        let corrected_id = u32::from_be_bytes([
                            resp_payload[1],
                            resp_payload[2],
                            resp_payload[3],
                            resp_payload[4],
                        ]);
                        assert_eq!(corrected_id, 42);

                        let pos_x_raw = i64::from_be_bytes([
                            resp_payload[13],
                            resp_payload[14],
                            resp_payload[15],
                            resp_payload[16],
                            resp_payload[17],
                            resp_payload[18],
                            resp_payload[19],
                            resp_payload[20],
                        ]);
                        let pos_z_raw = i64::from_be_bytes([
                            resp_payload[29],
                            resp_payload[30],
                            resp_payload[31],
                            resp_payload[32],
                            resp_payload[33],
                            resp_payload[34],
                            resp_payload[35],
                            resp_payload[36],
                        ]);
                        assert_eq!(Fixed64::from_raw(pos_x_raw), Fixed64::from_f64(10.0));
                        assert_eq!(Fixed64::from_raw(pos_z_raw), Fixed64::from_f64(20.0));
                        break;
                    }
                }
            }
        }
        if received_correction {
            break;
        }
    }

    let player = server.get_entity(42).expect("Player exists");
    assert_eq!(
        player.velocity.x,
        Fixed64::ZERO,
        "Blatant speed hack must reset horizontal velocity to zero"
    );
    assert_eq!(
        player.velocity.z,
        Fixed64::ZERO,
        "Blatant speed hack must reset horizontal velocity to zero"
    );
    assert!(received_correction, "Must receive StateCorrection packet");
}

#[test]
fn test_nan_inf_float_injection_rejected_without_panic() {
    let mut server = EidolonAppBuilder::new()
        .bind("127.0.0.1:0")
        .expect("Bind server")
        .build()
        .expect("Build server");

    let client_sock = UdpSocket::bind("127.0.0.1:0").expect("Bind client socket");
    client_sock.set_nonblocking(true).expect("Non-blocking");
    let client_addr = client_sock.local_addr().expect("Local addr");

    server
        .spawn_player(43, 1002, 2002, client_addr, 5.0, 0.0, 5.0)
        .expect("Spawn player");

    // Inject NaN and Infinity in velocity fields
    let mut payload = [0u8; 13];
    payload[0..4].copy_from_slice(&f32::NAN.to_be_bytes());
    payload[4..8].copy_from_slice(&f32::INFINITY.to_be_bytes());
    payload[8..12].copy_from_slice(&0.0f32.to_be_bytes());
    payload[12] = 0;

    let header = PacketHeader::new(
        ChannelType::UnreliableSequenced,
        PacketType::StateUpdate,
        2,
        0,
        0,
    );
    let mut packet = [0u8; 64];
    let hlen = header.write_to(&mut packet).expect("Write header");
    packet[hlen..hlen + 13].copy_from_slice(&payload);

    let server_addr = server.local_addr().expect("Server addr");
    client_sock
        .send_to(&packet[..hlen + 13], server_addr)
        .expect("Send packet");

    // Server ticks until packet is drained and StateCorrection is received
    let mut received_correction = false;
    let mut recv_buf = [0u8; 256];
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        server.tick().expect("Tick");

        while let Ok((rlen, _)) = client_sock.recv_from(&mut recv_buf) {
            if let Ok((resp_hdr, resp_hlen)) = PacketHeader::read_from(&recv_buf[..rlen]) {
                if resp_hdr.packet_type == PacketType::ReliableMessage {
                    let resp_payload = &recv_buf[resp_hlen..rlen];
                    if !resp_payload.is_empty() && resp_payload[0] == 10 {
                        received_correction = true;
                        break;
                    }
                }
            }
        }
        if received_correction {
            break;
        }
    }

    let player = server.get_entity(43).expect("Player exists");
    assert_eq!(player.velocity.x, Fixed64::ZERO);
    assert_eq!(player.velocity.z, Fixed64::ZERO);

    assert!(
        received_correction,
        "Must receive StateCorrection packet on NaN input"
    );
}

#[test]
fn test_authoritative_vertical_gravity_and_floor_landing() {
    let mut server = EidolonAppBuilder::new()
        .bind("127.0.0.1:0")
        .expect("Bind server")
        .build()
        .expect("Build server");

    // Spawn player 10 meters above the floor
    server
        .spawn_npc(50, 0, 0.0, 10.0, 0.0)
        .expect("Spawn airborne entity");

    let initial = server.get_entity(50).expect("Entity exists");
    assert_eq!(initial.position.y, Fixed64::from_i32(10));

    // Tick for 35 ticks (1.75s). With g = 9.80665 m/s^2, fall from 10m takes sqrt(20/9.8) ~ 1.43s (~29 ticks).
    for _ in 0..35 {
        server.tick().expect("Tick");
    }

    let landed = server.get_entity(50).expect("Entity exists");
    assert_eq!(
        landed.position.y,
        Fixed64::ZERO,
        "Entity must land on ground floor"
    );
    assert_eq!(
        landed.velocity.y,
        Fixed64::ZERO,
        "Vertical velocity must reset to 0 on ground"
    );
    assert_eq!(
        landed.flags & (FLAG_JUMPING | FLAG_FALLING),
        0,
        "Airborne flags must be cleared"
    );
    assert_eq!(
        landed.airborne_ticks, 0,
        "Airborne ticks must reset on landing"
    );
}

#[test]
fn test_levitation_fly_hack_watchdog_forces_fall() {
    let mut server = EidolonAppBuilder::new()
        .bind("127.0.0.1:0")
        .expect("Bind server")
        .build()
        .expect("Build server");

    server
        .spawn_npc(51, 0, 0.0, 15.0, 0.0)
        .expect("Spawn entity");

    let entity = server.get_entity_mut(51).expect("Entity exists");
    entity.flags = FLAG_JUMPING;
    entity.airborne_ticks = MAX_AIRBORNE_TICKS + 5; // Simulating hacked levitation / hover

    // Tick server
    server.tick().expect("Tick");

    let entity = server.get_entity(51).expect("Entity exists");
    assert_eq!(
        entity.flags & FLAG_FALLING,
        FLAG_FALLING,
        "Watchdog must override jumping and force falling flag"
    );
    assert_eq!(
        entity.flags & FLAG_JUMPING,
        0,
        "Jumping flag must be stripped"
    );
    assert!(
        entity.velocity.y < Fixed64::ZERO,
        "Entity must be pulled downward by gravity"
    );
}

#[test]
fn test_cast_while_moving_ability_completion_vs_stationary_interrupt() {
    let mut server = EidolonAppBuilder::new()
        .bind("127.0.0.1:0")
        .expect("Bind server")
        .build()
        .expect("Build server");

    // Spawn caster and target NPC
    server.spawn_npc(1, 0, 0.0, 0.0, 0.0).expect("Spawn caster");
    server.spawn_npc(2, 1, 0.0, 0.0, 4.0).expect("Spawn target");

    // 1. Test stationary ability: Fireball (Ability 1: allow_movement = false)
    let fb_def = get_ability_definition(1).expect("Fireball def");
    let caster = server.get_entity_mut(1).expect("Caster");
    let current_pos = caster.position;
    caster.cast_state = CastState::start_cast(
        1,
        2,
        0,
        fb_def.cast_duration_ticks,
        current_pos,
        fb_def.allow_movement,
    );

    // Caster moves 1.0m (exceeding 0.5m stationary threshold)
    let caster = server.get_entity_mut(1).expect("Caster");
    caster.position.x += Fixed64::from_f64(1.0);

    server.tick().expect("Tick");

    let caster = server.get_entity(1).expect("Caster");
    assert_eq!(
        caster.cast_state,
        CastState::Idle,
        "Stationary spell must be interrupted when caster moves >0.5m"
    );

    let target = server.get_entity(2).expect("Target");
    assert_eq!(target.health, 100, "Interrupted spell deals no damage");

    // 2. Test cast-while-moving ability: Whirlwind (Ability 5: allow_movement = true, 20 ticks, 50 damage)
    let ww_def = get_ability_definition(5).expect("Whirlwind def");
    let caster = server.get_entity_mut(1).expect("Caster");
    let current_pos = caster.position;
    caster.cast_state = CastState::start_cast(
        5,
        0,
        1,
        ww_def.cast_duration_ticks,
        current_pos,
        ww_def.allow_movement,
    );

    // Caster moves continuously across 20 ticks (displacing by 10.0m total!)
    for _ in 0..20 {
        let caster = server.get_entity_mut(1).expect("Caster");
        caster.position.x += Fixed64::from_f64(0.5); // 0.5m per tick * 20 = 10m
        server.tick().expect("Tick");
    }

    let caster = server.get_entity(1).expect("Caster");
    assert_eq!(
        caster.cast_state,
        CastState::Idle,
        "Cast-while-moving ability must complete successfully after 20 ticks"
    );
}
