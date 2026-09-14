//! Tier 1 Test Matrix: Mathematical precision, bit-for-bit deterministic parity,
//! and boundary extremes for eidolon-core.

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::kinematics::{
    extrapolate, should_dispatch_update, DeadReckoningConfig, KinematicState, FLAG_JUMPING,
    FLAG_SPRINTING, FLAG_WALKING,
};
use eidolon_core::quant::{
    QuantizedCellCoord, QuantizedYaw, CELL_HORIZONTAL_SIZE, CELL_VERTICAL_SIZE,
    MAX_QUANTIZED_HORIZONTAL, MAX_QUANTIZED_VERTICAL,
};
use eidolon_core::simd::Vec3Fix8x;

#[test]
fn test_deterministic_parity_10k_ticks() {
    let tick_duration = Fixed64::from_f64(0.05); // 20 Hz = 50ms per tick
    let initial_state = KinematicState {
        position: Vec3Fix::new(
            Fixed64::from_f64(12.3456),
            Fixed64::from_f64(1.2345),
            Fixed64::from_f64(56.7890),
        ),
        velocity: Vec3Fix::new(
            Fixed64::from_f64(3.5),
            Fixed64::from_f64(0.25),
            Fixed64::from_f64(-2.1),
        ),
        acceleration: Vec3Fix::new(
            Fixed64::from_f64(0.05),
            Fixed64::from_f64(-0.01),
            Fixed64::from_f64(0.02),
        ),
        yaw: QuantizedYaw::from_byte(42),
        angular_velocity: 1, // 1 discrete step per tick
        flags: FLAG_WALKING,
    };

    // 1. Lockstep parity: Server and client both executing 10,000 tick-by-tick steps
    let mut server_sim = initial_state;
    let mut client_sim = initial_state;
    for _ in 0..10_000 {
        server_sim = extrapolate(&server_sim, 1, tick_duration);
        client_sim = extrapolate(&client_sim, 1, tick_duration);
    }
    assert_eq!(
        server_sim.position, client_sim.position,
        "Lockstep simulation must be bit-exact to every bit"
    );
    assert_eq!(
        server_sim.velocity, client_sim.velocity,
        "Lockstep velocity must be bit-exact to every bit"
    );
    assert_eq!(
        server_sim.yaw, client_sim.yaw,
        "Lockstep yaw must be bit-exact to every bit"
    );

    // 2. Extrapolation parity: Server and client both computing direct extrapolation across 10,000 ticks
    let server_extrap = extrapolate(&initial_state, 10_000, tick_duration);
    let client_extrap = extrapolate(&initial_state, 10_000, tick_duration);
    assert_eq!(
        server_extrap.position, client_extrap.position,
        "Client and server closed-form extrapolation must be bit-for-bit identical"
    );
    assert_eq!(
        server_extrap.velocity, client_extrap.velocity,
        "Client and server closed-form velocity must be bit-for-bit identical"
    );

    // 3. Discrete Euler accumulation vs closed-form second-order polynomial drift
    // Over 10,000 ticks (500 seconds / 8.3 minutes of motion), accumulated truncation drift must be < 1.0mm
    let drift_x = (server_sim.position.x.to_f64() - server_extrap.position.x.to_f64()).abs();
    let drift_y = (server_sim.position.y.to_f64() - server_extrap.position.y.to_f64()).abs();
    let drift_z = (server_sim.position.z.to_f64() - server_extrap.position.z.to_f64()).abs();

    assert!(
        drift_x < 0.001,
        "X axis drift over 10,000 ticks must be sub-millimeter (< 1.0mm), got {}m",
        drift_x
    );
    assert!(
        drift_y < 0.001,
        "Y axis drift over 10,000 ticks must be sub-millimeter (< 1.0mm), got {}m",
        drift_y
    );
    assert!(
        drift_z < 0.001,
        "Z axis drift over 10,000 ticks must be sub-millimeter (< 1.0mm), got {}m",
        drift_z
    );
}

#[test]
fn test_fixed64_arithmetic_overflow_saturation() {
    let max = Fixed64::MAX;
    let min = Fixed64::MIN;
    let one = Fixed64::ONE;

    // Saturating addition and subtraction
    assert_eq!(max + one, max);
    assert_eq!(min - one, min);

    // Saturating multiplication
    let two = Fixed64::from_i32(2);
    assert_eq!(max * two, max);
    assert_eq!(min * two, min);

    // Division by zero must not panic, but saturate safely
    assert_eq!(Fixed64::from_i32(10) / Fixed64::ZERO, max);
    assert_eq!(Fixed64::from_i32(-10) / Fixed64::ZERO, min);
    assert_eq!(Fixed64::ZERO / Fixed64::ZERO, max);
}

#[test]
fn test_coordinate_quantization_extremes() {
    // Upper bound exactness (64m and 32m)
    let at_bound = QuantizedCellCoord::quantize(
        CELL_HORIZONTAL_SIZE,
        CELL_VERTICAL_SIZE,
        CELL_HORIZONTAL_SIZE,
    );
    assert_eq!(at_bound.x, MAX_QUANTIZED_HORIZONTAL);
    assert_eq!(at_bound.y, MAX_QUANTIZED_VERTICAL);
    assert_eq!(at_bound.z, MAX_QUANTIZED_HORIZONTAL);

    // Lower bound exactness (0m)
    let at_zero = QuantizedCellCoord::quantize(Fixed64::ZERO, Fixed64::ZERO, Fixed64::ZERO);
    assert_eq!(at_zero.x, 0);
    assert_eq!(at_zero.y, 0);
    assert_eq!(at_zero.z, 0);

    // Out-of-bounds negative positions must clamp safely to 0
    let negative = QuantizedCellCoord::quantize(
        Fixed64::from_i32(-50),
        Fixed64::from_i32(-10),
        Fixed64::from_i32(-999),
    );
    assert_eq!(negative.x, 0);
    assert_eq!(negative.y, 0);
    assert_eq!(negative.z, 0);

    // Out-of-bounds excessive positions must clamp safely to max
    let excessive = QuantizedCellCoord::quantize(
        Fixed64::from_i32(500),
        Fixed64::from_i32(200),
        Fixed64::from_i32(1000),
    );
    assert_eq!(excessive.x, MAX_QUANTIZED_HORIZONTAL);
    assert_eq!(excessive.y, MAX_QUANTIZED_VERTICAL);
    assert_eq!(excessive.z, MAX_QUANTIZED_HORIZONTAL);
}

#[test]
fn test_yaw_all_pair_shortest_arc_properties() {
    // Exhaustive test across all 256 discrete angles
    for a in 0..=255u8 {
        let yaw_a = QuantizedYaw::from_byte(a);

        // Self delta must be 0
        assert_eq!(yaw_a.shortest_arc_delta(yaw_a), 0);

        for b in 0..=255u8 {
            let yaw_b = QuantizedYaw::from_byte(b);
            let delta_ab = yaw_a.shortest_arc_delta(yaw_b);
            let delta_ba = yaw_b.shortest_arc_delta(yaw_a);

            // Shortest arc must be within [-128, 127]
            assert!(delta_ab >= -128);

            // Inverse symmetry holds everywhere except at the exact 180-degree antipodal point (-128)
            if delta_ab != -128 {
                assert_eq!(delta_ab, -delta_ba);
            }

            // Stepping from a to b by max 128 steps must reach b
            let reached = yaw_a.advance_toward(yaw_b, 128);
            assert_eq!(reached, yaw_b);
        }
    }
}

#[test]
fn test_dead_reckoning_update_triggers() {
    let config = DeadReckoningConfig::default();
    let tick_duration = Fixed64::from_f64(0.05);

    let state_a = KinematicState::stationary(Vec3Fix::ZERO, QuantizedYaw::NORTH);

    // Stationary entity: no updates dispatched for ticks 0..39
    for elapsed in 0..config.heartbeat_ticks {
        let dispatched = should_dispatch_update(&state_a, &state_a, elapsed, &config);
        assert!(
            !dispatched,
            "Stationary entity should not dispatch at tick {}",
            elapsed
        );
    }

    // Heartbeat update dispatched at tick 40
    assert!(
        should_dispatch_update(&state_a, &state_a, config.heartbeat_ticks, &config),
        "Heartbeat must dispatch update at tick threshold"
    );

    // Flag change (e.g. began jumping): immediate dispatch
    let mut state_jumping = state_a;
    state_jumping.flags = FLAG_JUMPING;
    assert!(
        should_dispatch_update(&state_jumping, &state_a, 1, &config),
        "Movement flag shift must trigger immediate update"
    );

    // Position divergence: moving 0.08m (> 0.05m deadband)
    let state_moved =
        KinematicState::stationary(Vec3Fix::from_f64(0.08, 0.0, 0.0), QuantizedYaw::NORTH);
    assert!(
        should_dispatch_update(&state_moved, &state_a, 1, &config),
        "Position divergence exceeding 5cm deadband must dispatch update"
    );

    // Heading divergence: turning 4 discrete steps (> 3 steps deadband)
    let state_turned = KinematicState::stationary(Vec3Fix::ZERO, QuantizedYaw::from_byte(4));
    assert!(
        should_dispatch_update(&state_turned, &state_a, 1, &config),
        "Heading divergence exceeding 3 steps must dispatch update"
    );

    // Kinematic constant velocity: server and client extrapolate identically -> zero updates
    let moving_state = KinematicState {
        position: Vec3Fix::ZERO,
        velocity: Vec3Fix::from_f64(5.0, 0.0, 0.0),
        acceleration: Vec3Fix::ZERO,
        yaw: QuantizedYaw::EAST,
        angular_velocity: 0,
        flags: FLAG_SPRINTING,
    };

    let extrapolated = extrapolate(&moving_state, 10, tick_duration);
    let server_sim = extrapolate(&moving_state, 10, tick_duration);
    assert!(
        !should_dispatch_update(&server_sim, &extrapolated, 10, &config),
        "Constant velocity extrapolation without divergence should not dispatch packets"
    );
}

#[test]
fn test_bitpacking_compression_efficiency() {
    // 7 bytes per transform delta = 56 bits
    // At 20 Hz, even with an active transform every tick (worst-case uncoalesced):
    // 20 * 7 bytes = 140 bytes/sec << 1,200 bytes/sec wire budget!
    let coord = QuantizedCellCoord::from_f64(15.25, 8.5, 42.125);
    let yaw = QuantizedYaw::from_degrees(270.0);
    let flags = FLAG_WALKING;

    let wire_bytes = coord.pack_with_yaw_and_flags(yaw, flags);
    assert_eq!(wire_bytes.len(), 7);

    let (recovered_coord, recovered_yaw, recovered_flags) =
        QuantizedCellCoord::unpack_with_yaw_and_flags(wire_bytes);

    assert_eq!(recovered_coord, coord);
    assert_eq!(recovered_yaw, yaw);
    assert_eq!(recovered_flags, flags);
}

#[test]
fn test_simd_8x_mathematical_precision_and_parity() {
    let dt = Fixed64::from_f64(0.05); // 20 Hz = 50ms per tick

    // Initialize 8 distinct entities
    let mut positions = [Vec3Fix::ZERO; 8];
    let mut velocities = [Vec3Fix::ZERO; 8];
    let mut scalar_positions = [Vec3Fix::ZERO; 8];

    for i in 0..8 {
        positions[i] = Vec3Fix::from_f64((i as f64) * 12.345, (i as f64) * -2.5, (i as f64) * 7.89);
        velocities[i] = Vec3Fix::from_f64((i as f64 + 1.0) * 2.5, 0.125, (i as f64 + 1.0) * -1.75);
        scalar_positions[i] = positions[i];
    }

    // Step 1,000 ticks in lockstep
    for _ in 0..1_000 {
        Vec3Fix8x::step_kinematics_chunk(&mut positions, &velocities, dt);
        for i in 0..8 {
            let disp = velocities[i] * dt;
            scalar_positions[i] += disp;
        }
    }

    // Verify bit-for-bit exact parity
    for i in 0..8 {
        assert_eq!(
            positions[i].x.raw(),
            scalar_positions[i].x.raw(),
            "SIMD chunk X coordinate mismatch at lane {}",
            i
        );
        assert_eq!(
            positions[i].y.raw(),
            scalar_positions[i].y.raw(),
            "SIMD chunk Y coordinate mismatch at lane {}",
            i
        );
        assert_eq!(
            positions[i].z.raw(),
            scalar_positions[i].z.raw(),
            "SIMD chunk Z coordinate mismatch at lane {}",
            i
        );
    }

    // Test 8-lane radius bitmask filtering
    let target = Vec3Fix::from_f64(50.0, 0.0, 50.0);
    let radius_sq = Fixed64::from_i32(2500); // 50m radius (2,500 m^2)

    let v8 = Vec3Fix8x::from_slice_8(&positions);
    let mask = v8.filter_within_radius(target, radius_sq);

    for (i, pos) in positions.iter().enumerate() {
        let dist_sq = pos.distance_squared(target);
        let expected_in_radius = dist_sq <= radius_sq;
        let actual_in_radius = (mask & (1 << i)) != 0;
        assert_eq!(
            actual_in_radius, expected_in_radius,
            "Lane {} radius mask parity mismatch: dist_sq = {:?}, radius_sq = {:?}",
            i, dist_sq, radius_sq
        );
    }
}
