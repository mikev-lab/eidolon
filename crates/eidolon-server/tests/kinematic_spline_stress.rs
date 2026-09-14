//! kinematic_spline_stress.rs: Tier 2 adversarial trajectory reconciliation suite.
//!
//! Validates 2nd-order kinematic acceleration prediction, quintic C2 Hermite spline smoothing,
//! and predictive jerk deadbands under synthetic 40% packet loss over 1,000 simulated entities.

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::kinematics::{
    extrapolate, should_dispatch_update, DeadReckoningConfig, KinematicState,
    QuinticHermiteSpline3D, FLAG_SPRINTING, FLAG_WALKING,
};
use eidolon_core::quant::QuantizedYaw;
use eidolon_world::soa_storage::{EntitySpawnParams, SoaEntityStorage};

/// Linear Congruential Generator for deterministic chaos testing without external dependencies.
struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u32(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (self.state >> 32) as u32
    }

    fn gen_bool(&mut self, probability_true: f64) -> bool {
        let threshold = (probability_true * 4294967295.0) as u32;
        self.next_u32() <= threshold
    }
}

#[test]
fn test_predictive_acceleration_deadbands_bandwidth_reduction() {
    // 1,000 entities simulated over 100 ticks (5.0 seconds at 20 Hz)
    // Compare standard 1st-order velocity deadband vs 2nd-order predictive acceleration deadband
    let entity_count = 1000;
    let ticks = 100;
    let tick_duration = Fixed64::from_f64(0.05);

    let mut storage = SoaEntityStorage::with_capacity(entity_count);
    let mut authoritative_states = Vec::with_capacity(entity_count);
    let mut client_extrapolations = Vec::with_capacity(entity_count);
    let mut last_dispatched_tick = vec![0u32; entity_count];

    let dr_config = DeadReckoningConfig::default();

    for i in 0..entity_count {
        let id = (i + 1) as u32;
        let pos = Vec3Fix::from_f64((i as f64) * 5.0, 0.0, (i as f64) * 2.0);
        // Entities are moving and accelerating (e.g. accelerating sprint along heading)
        let vel = Vec3Fix::from_f64(6.0, 0.0, 0.0);
        let accel = Vec3Fix::from_f64(0.8, 0.0, 0.0); // 0.8 m/s^2 forward acceleration
        let heading = QuantizedYaw::from_degrees(90.0);

        let params = EntitySpawnParams::with_acceleration(id, pos, vel, accel, heading);
        storage.spawn(params).expect("spawn");

        let state = KinematicState::with_acceleration(pos, vel, accel, heading, FLAG_SPRINTING);
        authoritative_states.push(state);
        client_extrapolations.push(state);
    }

    let mut predictive_packets_dispatched = 0usize;
    let mut naive_packets_dispatched = 0usize;

    for tick in 1..=ticks {
        // Step server world storage using 2nd-order kinematics
        storage.step_kinematics_2nd_order(tick_duration);

        for i in 0..entity_count {
            let id = (i + 1) as u32;
            let (pos, vel, accel, heading) =
                storage.get_transform_2nd_order(id).expect("transform");

            let current_auth =
                KinematicState::with_acceleration(pos, vel, accel, heading, FLAG_SPRINTING);
            authoritative_states[i] = current_auth;

            // Naive 1st-order client extrapolation (only uses velocity, ignores acceleration)
            let elapsed = tick - last_dispatched_tick[i];
            let naive_extrap = {
                let dt = tick_duration * Fixed64::from_i32(elapsed as i32);
                let mut s = client_extrapolations[i];
                s.position += s.velocity * dt;
                s
            };
            if should_dispatch_update(&current_auth, &naive_extrap, elapsed, &dr_config) {
                naive_packets_dispatched += 1;
            }

            // Predictive 2nd-order client extrapolation (uses quadratic extrapolation)
            let predictive_extrap = extrapolate(&client_extrapolations[i], elapsed, tick_duration);
            if should_dispatch_update(&current_auth, &predictive_extrap, elapsed, &dr_config) {
                predictive_packets_dispatched += 1;
                client_extrapolations[i] = current_auth;
                last_dispatched_tick[i] = tick;
            }
        }
    }

    // Assert that predictive 2nd-order acceleration deadbands cut packet transmissions by >50%
    let reduction_ratio =
        1.0 - (predictive_packets_dispatched as f64 / naive_packets_dispatched as f64);
    println!(
        "Kinematic Deadband Bandwidth Benchmark: Naive packets: {}, Predictive packets: {}, Reduction: {:.1}%",
        naive_packets_dispatched,
        predictive_packets_dispatched,
        reduction_ratio * 100.0
    );

    assert!(
        reduction_ratio >= 0.50,
        "Predictive acceleration deadbands must reduce packet egress by at least 50% (measured {:.1}%)",
        reduction_ratio * 100.0
    );
}

#[test]
fn test_quintic_spline_convergence_under_40_percent_packet_loss() {
    let mut rng = DeterministicRng::new(0xDEAD_BEEF_C001_CAFE);
    let packet_loss_rate = 0.40; // 40% synthetic UDP packet drop

    let tick_duration = 0.05; // 50ms (20 Hz)
    let total_ticks = 100;

    // Simulate an accelerating entity changing heading gradually
    let mut server_pos = (0.0f64, 0.0f64, 0.0f64);
    let mut server_vel = (5.0f64, 0.0f64, 0.0f64);
    let server_accel = (1.2f64, 0.0f64, 0.3f64); // 3D acceleration

    // Client state tracking
    let mut client_pos = server_pos;
    let mut client_vel = server_vel;
    let mut client_accel = server_accel;

    let mut active_spline: Option<QuinticHermiteSpline3D> = None;
    let mut spline_elapsed = 0.0f64;
    let mut max_position_divergence = 0.0f64;

    for tick in 1..=total_ticks {
        // Advance server authoritative simulation
        server_pos.0 +=
            server_vel.0 * tick_duration + 0.5 * server_accel.0 * tick_duration * tick_duration;
        server_pos.1 +=
            server_vel.1 * tick_duration + 0.5 * server_accel.1 * tick_duration * tick_duration;
        server_pos.2 +=
            server_vel.2 * tick_duration + 0.5 * server_accel.2 * tick_duration * tick_duration;

        server_vel.0 += server_accel.0 * tick_duration;
        server_vel.1 += server_accel.1 * tick_duration;
        server_vel.2 += server_accel.2 * tick_duration;

        // Packet transmission: simulated with 40% loss
        let packet_dropped = rng.gen_bool(packet_loss_rate);

        if !packet_dropped {
            // New authoritative packet arrives: initialize Quintic Hermite Spline
            let spline = QuinticHermiteSpline3D::new_f64(
                client_pos,
                client_vel,
                client_accel,
                server_pos,
                server_vel,
                server_accel,
                tick_duration,
            );
            assert!(
                !spline.is_teleport_snapped(),
                "Continuous motion must not trigger teleport snap"
            );
            active_spline = Some(spline);
            spline_elapsed = 0.0;
        }

        // Client evaluates motion
        if let Some(ref spline) = active_spline {
            spline_elapsed += tick_duration;
            client_pos = spline.sample_position(spline_elapsed);
            client_vel = spline.sample_velocity(spline_elapsed);
            client_accel = spline.sample_acceleration(spline_elapsed);

            if spline_elapsed >= spline.tau() {
                active_spline = None;
            }
        } else {
            // Dead reckoning extrapolation forward between dropped packets
            client_pos.0 +=
                client_vel.0 * tick_duration + 0.5 * client_accel.0 * tick_duration * tick_duration;
            client_pos.1 +=
                client_vel.1 * tick_duration + 0.5 * client_accel.1 * tick_duration * tick_duration;
            client_pos.2 +=
                client_vel.2 * tick_duration + 0.5 * client_accel.2 * tick_duration * tick_duration;

            client_vel.0 += client_accel.0 * tick_duration;
            client_vel.1 += client_accel.1 * tick_duration;
            client_vel.2 += client_accel.2 * tick_duration;
        }

        let dx = server_pos.0 - client_pos.0;
        let dy = server_pos.1 - client_pos.1;
        let dz = server_pos.2 - client_pos.2;
        let divergence = (dx * dx + dy * dy + dz * dz).sqrt();

        if divergence > max_position_divergence {
            max_position_divergence = divergence;
        }

        // Even under 40% packet loss, divergence must remain tightly bounded without visual snapping
        assert!(
            divergence < 0.15,
            "Tick {}: position divergence {:.4}m exceeded 0.15m bound under 40% loss",
            tick,
            divergence
        );
    }

    println!(
        "Adversarial Spline Convergence: 40% loss across {} ticks, max divergence: {:.4}m (bound: <0.15m)",
        total_ticks, max_position_divergence
    );
}

#[test]
fn test_adversarial_jerk_and_heading_deflection_immediate_dispatch() {
    let dr_config = DeadReckoningConfig::default();
    let tick_duration = Fixed64::from_f64(0.05);

    let base_state = KinematicState::with_acceleration(
        Vec3Fix::ZERO,
        Vec3Fix::from_f64(5.0, 0.0, 0.0),
        Vec3Fix::from_f64(1.0, 0.0, 0.0),
        QuantizedYaw::from_degrees(0.0),
        FLAG_WALKING,
    );

    let extrapolated = extrapolate(&base_state, 1, tick_duration);

    // Case 1: No jerk, same heading, same velocity extrapolation -> zero update
    assert!(
        !should_dispatch_update(&extrapolated, &extrapolated, 1, &dr_config),
        "Identical trajectory must not dispatch update"
    );

    // Case 2: Sudden acceleration jerk (e.g. emergency braking: accel becomes -3.0 m/s^2)
    // Jerk delta = |1.0 - (-3.0)| = 4.0 m/s^2 > 0.2 m/s^2 deadband
    let mut jerk_state = extrapolated;
    jerk_state.acceleration = Vec3Fix::from_f64(-3.0, 0.0, 0.0);
    assert!(
        should_dispatch_update(&jerk_state, &extrapolated, 1, &dr_config),
        "Sudden jerk exceeding acceleration deadband must immediately dispatch update"
    );

    // Case 3: Sudden heading deflection (> 2.5 degrees / 2 discrete steps)
    let mut turn_state = extrapolated;
    turn_state.yaw = QuantizedYaw::from_degrees(10.0); // 10 degrees > 2.81 degrees
    assert!(
        should_dispatch_update(&turn_state, &extrapolated, 1, &dr_config),
        "Sharp heading turn must immediately dispatch update"
    );

    // Case 4: Teleport displacement (> 10.0m snap threshold)
    let spline_teleport = QuinticHermiteSpline3D::new_f64(
        (0.0, 0.0, 0.0),
        (0.0, 0.0, 0.0),
        (0.0, 0.0, 0.0),
        (15.0, 0.0, 0.0), // 15m displacement > 10m
        (0.0, 0.0, 0.0),
        (0.0, 0.0, 0.0),
        0.05,
    );
    assert!(
        spline_teleport.is_teleport_snapped(),
        "Displacement >= 10m must activate teleport snap"
    );
    let sample = spline_teleport.sample_position(0.01);
    assert_eq!(
        sample.0, 15.0,
        "Teleport snap must place position directly at target"
    );
}
