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

    /// Maximum angular deviation in discrete yaw steps before triggering an update.
    /// Default: 3 discrete steps (approx. 4.2 degrees).
    pub heading_deadband_steps: u8,

    /// Maximum ticks between packets regardless of movement (heartbeat threshold).
    /// Default: 40 ticks (2.0 seconds at 20 Hz).
    pub heartbeat_ticks: u32,
}

impl Default for DeadReckoningConfig {
    fn default() -> Self {
        Self {
            // (0.05)^2 = 0.0025
            position_deadband_squared: Fixed64::from_f64(0.0025),
            // (0.1)^2 = 0.01
            velocity_deadband_squared: Fixed64::from_f64(0.01),
            heading_deadband_steps: 3,
            heartbeat_ticks: 40,
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
        let delta_fixed = Fixed64::from_i32(yaw_delta.abs() as i32);
        let step_fixed = delta_fixed * alpha_clamped;
        let step = (step_fixed + Fixed64::HALF).to_i32().clamp(0, 255) as u8;
        if step > 0 {
            client_state.yaw = client_state.yaw.advance_toward(authoritative.yaw, step);
        }
    }
}
