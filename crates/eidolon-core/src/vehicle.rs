//! High-speed vehicle and flight dead reckoning kinematics.
//!
//! Supports terrestrial speeders, hover swoop bikes, supercars, aircraft, and flying mounts
//! traveling at speeds up to 150 m/s (540 km/h) across continental terrain, with trajectory
//! projection for predictive boundary pre-handshakes.

use crate::fixed::{Fixed64, Vec3Fix};
use crate::quant::QuantizedYaw;

/// Classification of high-speed vehicles and flight transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VehicleType {
    /// Hover speeder (up to 70 m/s / 252 km/h).
    GroundSpeeder = 0,
    /// Ultra-fast agile swoop bike (up to 110 m/s / 396 km/h).
    SwoopBike = 1,
    /// Wheeled high-performance automobile (up to 90 m/s / 324 km/h).
    Supercar = 2,
    /// Atmospheric jet or aircraft (up to 150 m/s / 540 km/h).
    Aircraft = 3,
    /// Aerial creature flying mount (up to 60 m/s / 216 km/h).
    FlyingMount = 4,
}

impl VehicleType {
    /// Returns the maximum forward speed in meters per second.
    pub const fn max_speed_mps(self) -> f32 {
        match self {
            Self::GroundSpeeder => 70.0,
            Self::SwoopBike => 110.0,
            Self::Supercar => 90.0,
            Self::Aircraft => 150.0,
            Self::FlyingMount => 60.0,
        }
    }

    /// Returns the maximum forward acceleration in meters per second squared.
    pub const fn max_acceleration_mps2(self) -> f32 {
        match self {
            Self::GroundSpeeder => 25.0,
            Self::SwoopBike => 45.0,
            Self::Supercar => 35.0,
            Self::Aircraft => 50.0,
            Self::FlyingMount => 20.0,
        }
    }
}

/// Authoritative kinematic state machine for high-speed vehicles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VehicleKinematics {
    /// Vehicle classification.
    pub vehicle_type: VehicleType,
    /// Authoritative continuous position.
    pub position: Vec3Fix,
    /// Continuous 3D velocity vector in meters per second.
    pub velocity: Vec3Fix,
    /// Facing heading discrete angle.
    pub yaw: QuantizedYaw,
    /// Pitch angle (-90 to +90 discrete degrees).
    pub pitch: i8,
    /// Banking roll angle (-90 to +90 discrete degrees).
    pub roll: i8,
    /// Throttle percentage input (0 to 100).
    pub throttle: u8,
    /// Steering input (-100 = full left, +100 = full right).
    pub steering: i8,
}

impl VehicleKinematics {
    /// Creates a new vehicle kinematic state at rest.
    pub fn new(vehicle_type: VehicleType, position: Vec3Fix, yaw: QuantizedYaw) -> Self {
        Self {
            vehicle_type,
            position,
            velocity: Vec3Fix::ZERO,
            yaw,
            pitch: 0,
            roll: 0,
            throttle: 0,
            steering: 0,
        }
    }

    /// Updates vehicle velocity and position over simulation step `delta_time_secs`.
    pub fn step(&mut self, delta_time_secs: Fixed64) {
        let max_speed = Fixed64::from_f64(self.vehicle_type.max_speed_mps() as f64);
        let accel = Fixed64::from_f64(self.vehicle_type.max_acceleration_mps2() as f64);

        // Compute forward direction vector from yaw
        let yaw_rad = self.yaw.to_degrees().to_radians();
        let fwd_x = Fixed64::from_f64(yaw_rad.sin());
        let fwd_z = Fixed64::from_f64(yaw_rad.cos());

        // Target speed based on throttle
        let target_speed =
            (max_speed * Fixed64::from_i32(self.throttle as i32)) / Fixed64::from_i32(100);

        // Accelerate velocity towards forward vector * target_speed
        let target_vel_x = fwd_x * target_speed;
        let target_vel_z = fwd_z * target_speed;

        let dv_max = accel * delta_time_secs;

        let diff_x = target_vel_x - self.velocity.x;
        let diff_z = target_vel_z - self.velocity.z;

        self.velocity.x += diff_x.clamp(-dv_max, dv_max);
        self.velocity.z += diff_z.clamp(-dv_max, dv_max);

        // Update position: pos = pos + vel * dt
        self.position.x += self.velocity.x * delta_time_secs;
        self.position.z += self.velocity.z * delta_time_secs;
    }

    /// Projects the estimated future position `lookahead_secs` ahead.
    ///
    /// Used by predictive boundary seam monitors to pre-authenticate crossing nodes.
    pub fn project_trajectory(&self, lookahead_secs: Fixed64) -> Vec3Fix {
        Vec3Fix {
            x: self.position.x + (self.velocity.x * lookahead_secs),
            y: self.position.y + (self.velocity.y * lookahead_secs),
            z: self.position.z + (self.velocity.z * lookahead_secs),
        }
    }

    /// Projects the estimated position in milliseconds into the future.
    pub fn predict_position_in_ms(&self, ms: u32) -> Vec3Fix {
        let dt = Fixed64::from_f64(ms as f64 / 1000.0);
        self.project_trajectory(dt)
    }

    /// Computes the continuous scalar speed in meters per second.
    pub fn current_speed_mps(&self) -> Fixed64 {
        let sq = (self.velocity.x * self.velocity.x) + (self.velocity.z * self.velocity.z);
        // Fixed-point square root approximation via f64 conversion
        Fixed64::from_f64(sq.to_f64().sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vehicle_acceleration_and_speed_capping() {
        let mut vk = VehicleKinematics::new(
            VehicleType::Aircraft,
            Vec3Fix::ZERO,
            QuantizedYaw::from_degrees(0.0), // Facing +Z
        );

        vk.throttle = 100; // Full throttle
        let dt = Fixed64::from_f64(0.05); // 50ms tick

        // Accelerate over 100 ticks (5.0 seconds)
        for _ in 0..100 {
            vk.step(dt);
        }

        // Top speed for Aircraft is 150.0 m/s
        let speed = vk.current_speed_mps().to_f64();
        assert!(
            (speed - 150.0).abs() < 1.0,
            "Aircraft did not reach max speed: {speed}"
        );
    }

    #[test]
    fn test_predictive_trajectory_lookahead() {
        let mut vk = VehicleKinematics::new(
            VehicleType::SwoopBike,
            Vec3Fix::from_f64(100.0, 0.0, 100.0),
            QuantizedYaw::from_degrees(90.0), // Facing +X
        );

        // Set velocity to 100 m/s along +X
        vk.velocity = Vec3Fix::from_f64(100.0, 0.0, 0.0);

        // Lookahead 500ms (0.5s): expected delta X = 100 * 0.5 = 50.0m
        let predicted = vk.predict_position_in_ms(500);
        assert_eq!(predicted.x, Fixed64::from_f64(150.0));
        assert_eq!(predicted.z, Fixed64::from_f64(100.0));
    }
}
