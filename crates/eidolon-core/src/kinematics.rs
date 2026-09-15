//! Kinematics, deterministic dead reckoning extrapolation, and divergence detection.
//!
//! Implements intent-based extrapolation state machines, allowing server and client
//! to maintain bit-exact synchronization across dropped or un-transmitted packets.

use crate::fixed::{Fixed64, Vec3Fix};
use crate::quant::QuantizedYaw;

/// Entity movement flag: stationary / idle.
pub const FLAG_IDLE: u8 = 0;

/// Entity movement flag: walking.
pub const FLAG_WALKING: u8 = 1 << 0;

/// Entity movement flag: sprinting.
pub const FLAG_SPRINTING: u8 = 1 << 1;

/// Entity movement flag: airborne / jumping.
pub const FLAG_JUMPING: u8 = 1 << 2;

/// Entity movement flag: falling under gravity.
pub const FLAG_FALLING: u8 = 1 << 3;

/// Entity movement flag: immobilized / stunned.
pub const FLAG_IMMOBILIZED: u8 = 1 << 4;

/// Complete deterministic kinematic state of an entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct KinematicState {
    /// Global or cell-relative continuous position.
    pub position: Vec3Fix,
    /// Velocity vector in distance units per second.
    pub velocity: Vec3Fix,
    /// Acceleration / intent vector in distance units per second squared.
    pub acceleration: Vec3Fix,
    /// Discrete facing direction.
    pub yaw: QuantizedYaw,
    /// Discrete angular velocity (steps per tick).
    pub angular_velocity: i8,
    /// Bitmask of entity movement states (walking, sprinting, jumping).
    pub flags: u8,
}

impl KinematicState {
    /// Constructs a stationary kinematic state at the specified position.
    #[inline]
    pub const fn stationary(position: Vec3Fix, yaw: QuantizedYaw) -> Self {
        Self {
            position,
            velocity: Vec3Fix::ZERO,
            acceleration: Vec3Fix::ZERO,
            yaw,
            angular_velocity: 0,
            flags: FLAG_IDLE,
        }
    }

    /// Constructs a moving kinematic state with constant velocity.
    #[inline]
    pub const fn with_velocity(
        position: Vec3Fix,
        velocity: Vec3Fix,
        yaw: QuantizedYaw,
        flags: u8,
    ) -> Self {
        Self {
            position,
            velocity,
            acceleration: Vec3Fix::ZERO,
            yaw,
            angular_velocity: 0,
            flags,
        }
    }

    /// Constructs a moving kinematic state with velocity and acceleration.
    #[inline]
    pub const fn with_acceleration(
        position: Vec3Fix,
        velocity: Vec3Fix,
        acceleration: Vec3Fix,
        yaw: QuantizedYaw,
        flags: u8,
    ) -> Self {
        Self {
            position,
            velocity,
            acceleration,
            yaw,
            angular_velocity: 0,
            flags,
        }
    }

    /// Returns true if the entity has negligible velocity and zero acceleration.
    #[inline]
    pub fn is_stationary(self) -> bool {
        self.velocity.magnitude_squared() <= Fixed64::EPSILON
            && self.acceleration.magnitude_squared() <= Fixed64::EPSILON
    }
}

/// Configuration parameters for dead reckoning divergence thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadReckoningConfig {
    /// Maximum squared position divergence before triggering an update packet.
    /// Default: (0.05m)^2 = 0.0025 m^2 (5 centimeters).
    pub position_deadband_squared: Fixed64,

    /// Maximum squared velocity divergence before triggering an update packet.
    /// Default: (0.1m/s)^2 = 0.01 (m/s)^2.
    pub velocity_deadband_squared: Fixed64,

    /// Maximum squared acceleration (jerk) divergence before triggering an update packet.
    /// Default: (0.2m/s^2)^2 = 0.04 (m/s^2)^2.
    pub acceleration_deadband_squared: Fixed64,

    /// Maximum angular deviation in discrete yaw steps before triggering an update.
    /// Default: 2 discrete steps (approx. 2.81 degrees, <= 2.5 deg calibrated threshold).
    pub heading_deadband_steps: u8,

    /// Maximum ticks between packets regardless of movement (heartbeat threshold).
    /// Default: 60 ticks (3.0 seconds at 20 Hz).
    pub heartbeat_ticks: u32,
}

impl Default for DeadReckoningConfig {
    fn default() -> Self {
        Self {
            // (0.05)^2 = 0.0025
            position_deadband_squared: Fixed64::from_f64(0.0025),
            // (0.1)^2 = 0.01
            velocity_deadband_squared: Fixed64::from_f64(0.01),
            // (0.2)^2 = 0.04
            acceleration_deadband_squared: Fixed64::from_f64(0.04),
            heading_deadband_steps: 2,
            heartbeat_ticks: 60,
        }
    }
}

/// Extrapolates kinematic motion across a given duration using second-order kinematics.
///
/// Mathematical model:
/// - P(t) = P_0 + V_0 * dt + 0.5 * A_0 * dt^2
/// - V(t) = V_0 + A_0 * dt
/// - Yaw(t) = Yaw_0 + angular_vel * delta_ticks
pub fn extrapolate(
    state: &KinematicState,
    delta_ticks: u32,
    tick_duration: Fixed64,
) -> KinematicState {
    if delta_ticks == 0 {
        return *state;
    }

    let dt = tick_duration * Fixed64::from_i32(delta_ticks as i32);
    let half_dt_sq = dt * dt * Fixed64::HALF;

    let displacement = (state.velocity * dt) + (state.acceleration * half_dt_sq);
    let new_pos = state.position + displacement;
    let new_vel = state.velocity + (state.acceleration * dt);

    let yaw_delta_steps = (state.angular_velocity as i32) * (delta_ticks as i32);
    let new_yaw = QuantizedYaw::from_byte(state.yaw.as_byte().wrapping_add(yaw_delta_steps as u8));

    KinematicState {
        position: new_pos,
        velocity: new_vel,
        acceleration: state.acceleration,
        yaw: new_yaw,
        angular_velocity: state.angular_velocity,
        flags: state.flags,
    }
}

/// Evaluates whether an authoritative state has diverged from an extrapolated prediction
/// beyond configured deadbands, dictating whether a wire transform packet must be dispatched.
pub fn should_dispatch_update(
    authoritative: &KinematicState,
    extrapolated: &KinematicState,
    elapsed_ticks: u32,
    config: &DeadReckoningConfig,
) -> bool {
    // Heartbeat check: must transmit periodically even if motionless
    if elapsed_ticks >= config.heartbeat_ticks {
        return true;
    }

    // State change check: discrete flags changed (e.g. started jumping or sprinting)
    if authoritative.flags != extrapolated.flags {
        return true;
    }

    // Position divergence check
    let pos_dist_sq = authoritative
        .position
        .distance_squared(extrapolated.position);
    if pos_dist_sq > config.position_deadband_squared {
        return true;
    }

    // Heading divergence check
    let yaw_delta = authoritative
        .yaw
        .shortest_arc_delta(extrapolated.yaw)
        .unsigned_abs();
    if yaw_delta > config.heading_deadband_steps {
        return true;
    }

    // Velocity divergence check
    let vel_dist_sq = authoritative
        .velocity
        .distance_squared(extrapolated.velocity);
    if vel_dist_sq > config.velocity_deadband_squared {
        return true;
    }

    // Acceleration divergence check (jerk deadband)
    let accel_dist_sq = authoritative
        .acceleration
        .distance_squared(extrapolated.acceleration);
    if accel_dist_sq > config.acceleration_deadband_squared {
        return true;
    }

    false
}

/// Smoothly reconciles client predicted state toward an authoritative server update,
/// mitigating rubber-banding artifacts over a blending factor alpha [0.0, 1.0].
pub fn reconcile_smooth(
    client_state: &mut KinematicState,
    authoritative: &KinematicState,
    alpha: Fixed64,
) {
    let alpha_clamped = alpha.clamp(Fixed64::ZERO, Fixed64::ONE);
    let pos_error = authoritative.position - client_state.position;
    client_state.position += pos_error * alpha_clamped;

    let vel_error = authoritative.velocity - client_state.velocity;
    client_state.velocity += vel_error * alpha_clamped;

    client_state.acceleration = authoritative.acceleration;
    client_state.angular_velocity = authoritative.angular_velocity;
    client_state.flags = authoritative.flags;

    // Advance heading toward authoritative yaw using exact fixed-point smoothing
    let yaw_delta = client_state.yaw.shortest_arc_delta(authoritative.yaw);
    if yaw_delta != 0 {
        let delta_fixed = Fixed64::from_i32(yaw_delta.unsigned_abs() as i32);
        let step_fixed = delta_fixed * alpha_clamped;
        let step = (step_fixed + Fixed64::HALF).to_i32().clamp(0, 255) as u8;
        if step > 0 {
            client_state.yaw = client_state.yaw.advance_toward(authoritative.yaw, step);
        }
    }
}

/// Continuous C2 Quintic Hermite Spline for 3D position, velocity, and acceleration continuity.
///
/// Blends from an initial kinematic state (P0, V0, A0) to a target kinematic state (P1, V1, A1)
/// across a smoothing duration `tau` (default: 50ms / 0.05s). Guarantees continuity of position,
/// velocity, and acceleration without jerk impulse on update boundaries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuinticHermiteSpline3D {
    p0: (f64, f64, f64),
    v0: (f64, f64, f64),
    a0: (f64, f64, f64),
    p1: (f64, f64, f64),
    v1: (f64, f64, f64),
    a1: (f64, f64, f64),
    tau: f64,
    teleport_snapped: bool,
}

impl QuinticHermiteSpline3D {
    /// Teleport distance threshold (10.0m) beyond which spline smoothing is bypassed to prevent smearing.
    pub const TELEPORT_DISTANCE_THRESHOLD: f64 = 10.0;

    /// Constructs a new Quintic Hermite Spline from fixed-point vectors.
    pub fn new(
        p0: Vec3Fix,
        v0: Vec3Fix,
        a0: Vec3Fix,
        p1: Vec3Fix,
        v1: Vec3Fix,
        a1: Vec3Fix,
        tau: f64,
    ) -> Self {
        Self::new_f64(
            p0.to_f64(),
            v0.to_f64(),
            a0.to_f64(),
            p1.to_f64(),
            v1.to_f64(),
            a1.to_f64(),
            tau,
        )
    }

    /// Constructs a new Quintic Hermine Spline from floating-point coordinate tuples.
    pub fn new_f64(
        p0: (f64, f64, f64),
        v0: (f64, f64, f64),
        a0: (f64, f64, f64),
        p1: (f64, f64, f64),
        v1: (f64, f64, f64),
        a1: (f64, f64, f64),
        tau: f64,
    ) -> Self {
        let dx = p1.0 - p0.0;
        let dy = p1.1 - p0.1;
        let dz = p1.2 - p0.2;
        let dist_sq = dx * dx + dy * dy + dz * dz;
        let teleport_snapped =
            dist_sq >= Self::TELEPORT_DISTANCE_THRESHOLD * Self::TELEPORT_DISTANCE_THRESHOLD;
        let tau_safe = if tau <= 0.0001 { 0.05 } else { tau };

        Self {
            p0,
            v0,
            a0,
            p1,
            v1,
            a1,
            tau: tau_safe,
            teleport_snapped,
        }
    }

    /// Returns true if the distance between P0 and P1 exceeded the teleport threshold.
    #[inline]
    pub fn is_teleport_snapped(&self) -> bool {
        self.teleport_snapped
    }

    /// Returns the blending duration tau in seconds.
    #[inline]
    pub fn tau(&self) -> f64 {
        self.tau
    }

    /// Samples continuous position (x, y, z) at elapsed time `delta_seconds`.
    pub fn sample_position(&self, delta_seconds: f64) -> (f64, f64, f64) {
        if self.teleport_snapped {
            let dt = delta_seconds.max(0.0);
            return (
                self.p1.0 + self.v1.0 * dt + 0.5 * self.a1.0 * dt * dt,
                self.p1.1 + self.v1.1 * dt + 0.5 * self.a1.1 * dt * dt,
                self.p1.2 + self.v1.2 * dt + 0.5 * self.a1.2 * dt * dt,
            );
        }

        if delta_seconds <= 0.0 {
            return self.p0;
        }

        if delta_seconds >= self.tau {
            let dt = delta_seconds - self.tau;
            return (
                self.p1.0 + self.v1.0 * dt + 0.5 * self.a1.0 * dt * dt,
                self.p1.1 + self.v1.1 * dt + 0.5 * self.a1.1 * dt * dt,
                self.p1.2 + self.v1.2 * dt + 0.5 * self.a1.2 * dt * dt,
            );
        }

        let u = delta_seconds / self.tau;
        let u2 = u * u;
        let u3 = u2 * u;
        let u4 = u3 * u;
        let u5 = u4 * u;

        // Quintic Hermite basis functions:
        // h0(u) = 1 - 10u^3 + 15u^4 - 6u^5
        // h1(u) = u - 6u^3 + 8u^4 - 3u^5
        // h2(u) = 0.5u^2 - 1.5u^3 + 1.5u^4 - 0.5u^5
        // h3(u) = 0.5u^3 - u^4 + 0.5u^5
        // h4(u) = -4u^3 + 7u^4 - 3u^5
        // h5(u) = 10u^3 - 15u^4 + 6u^5
        let h0 = 1.0 - 10.0 * u3 + 15.0 * u4 - 6.0 * u5;
        let h1 = u - 6.0 * u3 + 8.0 * u4 - 3.0 * u5;
        let h2 = 0.5 * u2 - 1.5 * u3 + 1.5 * u4 - 0.5 * u5;
        let h3 = 0.5 * u3 - u4 + 0.5 * u5;
        let h4 = -4.0 * u3 + 7.0 * u4 - 3.0 * u5;
        let h5 = 10.0 * u3 - 15.0 * u4 + 6.0 * u5;

        let tau = self.tau;
        let tau2 = tau * tau;

        let sample_axis = |p0: f64, v0: f64, a0: f64, p1: f64, v1: f64, a1: f64| -> f64 {
            h0 * p0
                + h1 * (tau * v0)
                + h2 * (tau2 * a0)
                + h3 * (tau2 * a1)
                + h4 * (tau * v1)
                + h5 * p1
        };

        (
            sample_axis(
                self.p0.0, self.v0.0, self.a0.0, self.p1.0, self.v1.0, self.a1.0,
            ),
            sample_axis(
                self.p0.1, self.v0.1, self.a0.1, self.p1.1, self.v1.1, self.a1.1,
            ),
            sample_axis(
                self.p0.2, self.v0.2, self.a0.2, self.p1.2, self.v1.2, self.a1.2,
            ),
        )
    }

    /// Samples continuous velocity (vx, vy, vz) at elapsed time `delta_seconds`.
    pub fn sample_velocity(&self, delta_seconds: f64) -> (f64, f64, f64) {
        if self.teleport_snapped {
            let dt = delta_seconds.max(0.0);
            return (
                self.v1.0 + self.a1.0 * dt,
                self.v1.1 + self.a1.1 * dt,
                self.v1.2 + self.a1.2 * dt,
            );
        }

        if delta_seconds <= 0.0 {
            return self.v0;
        }

        if delta_seconds >= self.tau {
            let dt = delta_seconds - self.tau;
            return (
                self.v1.0 + self.a1.0 * dt,
                self.v1.1 + self.a1.1 * dt,
                self.v1.2 + self.a1.2 * dt,
            );
        }

        let u = delta_seconds / self.tau;
        let u2 = u * u;
        let u3 = u2 * u;
        let u4 = u3 * u;

        // Derivatives of basis functions:
        let dh0 = -30.0 * u2 + 60.0 * u3 - 30.0 * u4;
        let dh1 = 1.0 - 18.0 * u2 + 32.0 * u3 - 15.0 * u4;
        let dh2 = u - 4.5 * u2 + 6.0 * u3 - 2.5 * u4;
        let dh3 = 1.5 * u2 - 4.0 * u3 + 2.5 * u4;
        let dh4 = -12.0 * u2 + 28.0 * u3 - 15.0 * u4;
        let dh5 = 30.0 * u2 - 60.0 * u3 + 30.0 * u4;

        let tau = self.tau;
        let tau2 = tau * tau;

        let sample_axis_vel = |p0: f64, v0: f64, a0: f64, p1: f64, v1: f64, a1: f64| -> f64 {
            (dh0 * p0
                + dh1 * (tau * v0)
                + dh2 * (tau2 * a0)
                + dh3 * (tau2 * a1)
                + dh4 * (tau * v1)
                + dh5 * p1)
                / tau
        };

        (
            sample_axis_vel(
                self.p0.0, self.v0.0, self.a0.0, self.p1.0, self.v1.0, self.a1.0,
            ),
            sample_axis_vel(
                self.p0.1, self.v0.1, self.a0.1, self.p1.1, self.v1.1, self.a1.1,
            ),
            sample_axis_vel(
                self.p0.2, self.v0.2, self.a0.2, self.p1.2, self.v1.2, self.a1.2,
            ),
        )
    }

    /// Samples continuous acceleration (ax, ay, az) at elapsed time `delta_seconds`.
    pub fn sample_acceleration(&self, delta_seconds: f64) -> (f64, f64, f64) {
        if self.teleport_snapped || delta_seconds >= self.tau {
            return self.a1;
        }

        if delta_seconds <= 0.0 {
            return self.a0;
        }

        let u = delta_seconds / self.tau;
        let u2 = u * u;
        let u3 = u2 * u;

        // Second derivatives of basis functions:
        let d2h0 = -60.0 * u + 180.0 * u2 - 120.0 * u3;
        let d2h1 = -36.0 * u + 96.0 * u2 - 60.0 * u3;
        let d2h2 = 1.0 - 9.0 * u + 18.0 * u2 - 10.0 * u3;
        let d2h3 = 3.0 * u - 12.0 * u2 + 10.0 * u3;
        let d2h4 = -24.0 * u + 84.0 * u2 - 60.0 * u3;
        let d2h5 = 60.0 * u - 180.0 * u2 + 120.0 * u3;

        let tau = self.tau;
        let tau2 = tau * tau;

        let sample_axis_acc = |p0: f64, v0: f64, a0: f64, p1: f64, v1: f64, a1: f64| -> f64 {
            (d2h0 * p0
                + d2h1 * (tau * v0)
                + d2h2 * (tau2 * a0)
                + d2h3 * (tau2 * a1)
                + d2h4 * (tau * v1)
                + d2h5 * p1)
                / tau2
        };

        (
            sample_axis_acc(
                self.p0.0, self.v0.0, self.a0.0, self.p1.0, self.v1.0, self.a1.0,
            ),
            sample_axis_acc(
                self.p0.1, self.v0.1, self.a0.1, self.p1.1, self.v1.1, self.a1.1,
            ),
            sample_axis_acc(
                self.p0.2, self.v0.2, self.a0.2, self.p1.2, self.v1.2, self.a1.2,
            ),
        )
    }

    /// Samples continuous position converted to deterministic fixed-point Vec3Fix.
    pub fn sample_position_fixed(&self, delta_seconds: f64) -> Vec3Fix {
        let (x, y, z) = self.sample_position(delta_seconds);
        Vec3Fix::from_f64(x, y, z)
    }
}
