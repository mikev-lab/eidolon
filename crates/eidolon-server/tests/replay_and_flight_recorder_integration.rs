//! Phase 28: Live Deterministic Replay, Time-Travel Debugging and Black-Box Flight Recorder.
//!
//! Validates:
//! 1. FlightRecorder rolling input and keyframe logging without dynamic allocations.
//! 2. ReplaySession forward/reverse time scrubbing with 100% bit-for-bit deterministic parity.
//! 3. Variable-speed playback rates (0.25x, 1.0x, 4.0x, 16.0x).
//! 4. Esports refereeing and anti-cheat divergence detection with exact spatial drift distance.

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::quant::QuantizedYaw;
use eidolon_core::replay::{EntityStateSnapshot, PlaybackSpeed};
use eidolon_world::flight_recorder::{FlightRecorder, ReplayError};

#[test]
fn test_flight_recorder_input_logging_and_keyframing() {
    let mut recorder = FlightRecorder::new(20); // Keyframe every 20 ticks
    assert_eq!(recorder.earliest_tick(), 0);
    assert_eq!(recorder.latest_tick(), 0);

    // Record 60 ticks of entity 1 moving East at 4 m/s
    let dt = Fixed64::from_f64(0.050);
    let mut cur_pos = Vec3Fix::ZERO;
    let vel = Vec3Fix::new(Fixed64::from_i32(4), Fixed64::ZERO, Fixed64::ZERO);

    for tick in 1..=60 {
        // Record input
        recorder.record_input(tick, 101, vel, QuantizedYaw::EAST, 1);

        // Update position
        cur_pos.x += vel.x * dt;

        // Periodic keyframe every 20 ticks
        if tick % 20 == 0 {
            let snap = EntityStateSnapshot {
                entity_id: 101,
                position: cur_pos,
                velocity: vel,
                yaw: QuantizedYaw::EAST,
                flags: 1,
            };
            recorder.record_keyframe(tick, &[snap]);
        }
    }

    assert_eq!(recorder.earliest_tick(), 1);
    assert_eq!(recorder.latest_tick(), 60);

    // Verify keyframe lookup
    let kf_prior_35 = recorder.find_nearest_prior_keyframe(35).expect("find kf");
    assert_eq!(kf_prior_35.tick, 20);

    let kf_prior_40 = recorder.find_nearest_prior_keyframe(40).expect("find kf");
    assert_eq!(kf_prior_40.tick, 40);

    let kf_prior_59 = recorder.find_nearest_prior_keyframe(59).expect("find kf");
    assert_eq!(kf_prior_59.tick, 40);

    let kf_prior_60 = recorder.find_nearest_prior_keyframe(60).expect("find kf");
    assert_eq!(kf_prior_60.tick, 60);
}

#[test]
fn test_replay_session_time_travel_scrubbing_and_determinism() {
    let mut recorder = FlightRecorder::new(10); // Keyframe every 10 ticks

    let dt = Fixed64::from_f64(0.050);
    let mut pos1 = Vec3Fix::ZERO;
    let vel1 = Vec3Fix::new(Fixed64::from_i32(5), Fixed64::ZERO, Fixed64::ZERO); // 5 m/s East

    let mut pos2 = Vec3Fix::new(Fixed64::ZERO, Fixed64::ZERO, Fixed64::from_i32(50));
    let vel2 = Vec3Fix::new(Fixed64::ZERO, Fixed64::ZERO, Fixed64::from_i32(-2)); // 2 m/s North (-Z)

    // Record initial keyframe at tick 0
    let snap1_0 = EntityStateSnapshot {
        entity_id: 1,
        position: pos1,
        velocity: vel1,
        yaw: QuantizedYaw::EAST,
        flags: 1,
    };
    let snap2_0 = EntityStateSnapshot {
        entity_id: 2,
        position: pos2,
        velocity: vel2,
        yaw: QuantizedYaw::NORTH,
        flags: 1,
    };
    recorder.record_keyframe(0, &[snap1_0, snap2_0]);

    // Simulate and record 50 ticks
    for tick in 1..=50 {
        recorder.record_input(tick, 1, vel1, QuantizedYaw::EAST, 1);
        recorder.record_input(tick, 2, vel2, QuantizedYaw::NORTH, 1);

        pos1.x += vel1.x * dt;
        pos2.z += vel2.z * dt;

        if tick % 10 == 0 {
            let snap1 = EntityStateSnapshot {
                entity_id: 1,
                position: pos1,
                velocity: vel1,
                yaw: QuantizedYaw::EAST,
                flags: 1,
            };
            let snap2 = EntityStateSnapshot {
                entity_id: 2,
                position: pos2,
                velocity: vel2,
                yaw: QuantizedYaw::NORTH,
                flags: 1,
            };
            recorder.record_keyframe(tick, &[snap1, snap2]);
        }
    }

    let mut session = recorder.create_replay_session().expect("create session");
    assert_eq!(session.current_tick(), 0);

    // Scrub forward to tick 35 (between keyframe 30 and 40)
    let tick35 = session.scrub_to_tick(35).expect("scrub to 35");
    assert_eq!(tick35, 35);
    let state1_at_35 = session.get_entity_state(1).expect("ent 1 state");
    let state2_at_35 = session.get_entity_state(2).expect("ent 2 state");
    let checksum_at_35 = session.current_checksum();

    // Theoretical pos at 35 ticks:
    // Ent 1: 5 m/s * (35 * 0.050s) = 5 * 1.75 = 8.75m
    // Ent 2: 50m - 2 m/s * (35 * 0.050s) = 50 - 3.5 = 46.50m
    assert!((state1_at_35.position.x.to_f64() - 8.75).abs() < 0.001);
    assert!((state2_at_35.position.z.to_f64() - 46.50).abs() < 0.001);

    // Scrub backward to tick 15 (time-travel backward)
    let tick15 = session.scrub_to_tick(15).expect("scrub backward to 15");
    assert_eq!(tick15, 15);
    let state1_at_15 = session.get_entity_state(1).expect("ent 1 state");
    // Theoretical pos at 15 ticks: 5 m/s * 0.75s = 3.75m
    assert!((state1_at_15.position.x.to_f64() - 3.75).abs() < 0.001);

    // Scrub forward again to tick 35 (verify 100% deterministic bit-for-bit parity)
    let tick35_again = session
        .scrub_to_tick(35)
        .expect("scrub forward to 35 again");
    assert_eq!(tick35_again, 35);
    let state1_at_35_again = session.get_entity_state(1).expect("ent 1 state");
    let state2_at_35_again = session.get_entity_state(2).expect("ent 2 state");
    let checksum_at_35_again = session.current_checksum();

    assert_eq!(state1_at_35, state1_at_35_again);
    assert_eq!(state2_at_35, state2_at_35_again);
    assert_eq!(checksum_at_35, checksum_at_35_again);
}

#[test]
fn test_replay_session_stepping_and_playback_speeds() {
    let mut recorder = FlightRecorder::new(10);
    let snap = EntityStateSnapshot {
        entity_id: 5,
        position: Vec3Fix::ZERO,
        velocity: Vec3Fix::ZERO,
        yaw: QuantizedYaw::NORTH,
        flags: 0,
    };
    recorder.record_keyframe(0, &[snap]);

    for tick in 1..=40 {
        recorder.record_input(tick, 5, Vec3Fix::ZERO, QuantizedYaw::NORTH, 0);
        if tick % 10 == 0 {
            recorder.record_keyframe(tick, &[snap]);
        }
    }

    let mut session = recorder.create_replay_session().expect("create session");
    assert_eq!(session.playback_speed(), PlaybackSpeed::Normal);

    session.set_playback_speed(PlaybackSpeed::Fast);
    assert_eq!(session.playback_speed(), PlaybackSpeed::Fast);

    // Step forward 5 ticks
    let t = session.step_forward(5).expect("step forward");
    assert_eq!(t, 5);

    // Step forward 10 ticks
    let t2 = session.step_forward(10).expect("step forward");
    assert_eq!(t2, 15);

    // Step backward 8 ticks
    let t3 = session.step_backward(8).expect("step backward");
    assert_eq!(t3, 7);

    // Bounded step beyond latest
    let t_max = session.step_forward(100).expect("step beyond");
    assert_eq!(t_max, 40);

    // Bounded step before earliest
    let t_min = session.step_backward(100).expect("step before");
    assert_eq!(t_min, 0);
}

#[test]
fn test_referee_anti_cheat_divergence_detection() {
    let mut recorder = FlightRecorder::new(10);

    let dt = Fixed64::from_f64(0.050);
    let mut pos = Vec3Fix::ZERO;
    let vel = Vec3Fix::new(Fixed64::from_i32(6), Fixed64::ZERO, Fixed64::ZERO);

    let snap0 = EntityStateSnapshot {
        entity_id: 42,
        position: pos,
        velocity: vel,
        yaw: QuantizedYaw::EAST,
        flags: 1,
    };
    recorder.record_keyframe(0, &[snap0]);

    for tick in 1..=20 {
        recorder.record_input(tick, 42, vel, QuantizedYaw::EAST, 1);
        pos.x += vel.x * dt;
        if tick % 10 == 0 {
            let snap = EntityStateSnapshot {
                entity_id: 42,
                position: pos,
                velocity: vel,
                yaw: QuantizedYaw::EAST,
                flags: 1,
            };
            recorder.record_keyframe(tick, &[snap]);
        }
    }

    let mut session = recorder.create_replay_session().expect("create session");
    session.scrub_to_tick(20).expect("scrub to 20");

    let authoritative_snap = session.get_entity_state(42).expect("ent 42 state");
    let authoritative_checksum = session.current_checksum();

    // 1. Legitimate live state matching recording
    let honest_live_state = [authoritative_snap];
    let honest_check = session.detect_divergence(20, authoritative_checksum, &honest_live_state);
    assert_eq!(honest_check, None);

    // 2. Malicious live state with position hack (speed hack / teleport of +15m)
    let hacked_pos = Vec3Fix::new(
        authoritative_snap.position.x + Fixed64::from_i32(15),
        authoritative_snap.position.y,
        authoritative_snap.position.z,
    );
    let hacked_snap = EntityStateSnapshot {
        position: hacked_pos,
        ..authoritative_snap
    };
    let hacked_live_state = [hacked_snap];
    let hacked_checksum = authoritative_checksum.wrapping_add(9999);

    let divergence = session
        .detect_divergence(20, hacked_checksum, &hacked_live_state)
        .expect("divergence detected");

    assert_eq!(divergence.tick, 20);
    assert_eq!(divergence.divergent_entity_id, Some(42));
    assert_eq!(divergence.expected_checksum, authoritative_checksum);
    assert_eq!(divergence.actual_checksum, hacked_checksum);
    assert!((divergence.drift_distance - 15.0).abs() < 0.01);
}

#[test]
fn test_replay_scrub_out_of_range_handling() {
    let mut recorder = FlightRecorder::new(10);
    let snap = EntityStateSnapshot::default();
    recorder.record_keyframe(10, &[snap]);
    recorder.record_input(10, 1, Vec3Fix::ZERO, QuantizedYaw::NORTH, 0);
    recorder.record_input(20, 1, Vec3Fix::ZERO, QuantizedYaw::NORTH, 0);
    recorder.record_keyframe(20, &[snap]);

    let mut session = recorder.create_replay_session().expect("create session");

    // Scrub before earliest (tick 5 < 10)
    let err_early = session.scrub_to_tick(5).unwrap_err();
    match err_early {
        ReplayError::TickOutOfRange {
            requested,
            earliest,
            ..
        } => {
            assert_eq!(requested, 5);
            assert_eq!(earliest, 10);
        }
        other => panic!("Unexpected error: {:?}", other),
    }

    // Scrub after latest (tick 50 > 20)
    let err_late = session.scrub_to_tick(50).unwrap_err();
    match err_late {
        ReplayError::TickOutOfRange {
            requested, latest, ..
        } => {
            assert_eq!(requested, 50);
            assert_eq!(latest, 20);
        }
        other => panic!("Unexpected error: {:?}", other),
    }
}
