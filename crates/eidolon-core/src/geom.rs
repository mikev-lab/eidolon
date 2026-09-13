//! Server-authoritative geometric spatial collision and hit-testing primitives.
//!
//! Provides deterministic fixed-point collision tests (Spheres, Cones, and Oriented Boxes)
//! for abilities, melee swings, and area-of-effect (AoE) hit validation.
//!
//! Designed with mechanical sympathy: exploits dot products and squared magnitudes to
//! eliminate expensive square roots and inverse trigonometric evaluations in the hot loop.

use crate::fixed::{Fixed64, Vec3Fix};
use crate::quant::QuantizedYaw;

/// Server-authoritative geometric collision test engine.
pub struct SpatialGeometry;

impl SpatialGeometry {
    /// Tests if a target point lies within a spherical radius of an origin point.
    ///
    /// Evaluates: |target - origin|^2 <= radius^2 without computing square roots.
    #[inline]
    pub fn test_sphere(origin: Vec3Fix, radius: Fixed64, target: Vec3Fix) -> bool {
        if radius <= Fixed64::ZERO {
            return false;
        }
        let diff = target - origin;
        diff.magnitude_squared() <= radius * radius
    }

    /// Tests if a target point lies within a forward-facing circular cone (sector).
    ///
    /// - `origin`: Apex point of the cone (caster position).
    /// - `forward_yaw`: Discrete horizontal heading direction.
    /// - `half_angle_deg`: Half of the cone's total angular spread in degrees (e.g. 30.0 for a 60-degree cone).
    /// - `max_distance`: Maximum horizontal range of the cone.
    /// - `max_height`: Maximum allowed vertical distance (Y-axis) from origin.
    /// - `target`: Evaluated entity position.
    ///
    /// Uses algebraic squaring to avoid square roots:
    /// (F . D)^2 >= |D|^2 * cos^2(half_angle) when (F . D) > 0.
    pub fn test_cone(
        origin: Vec3Fix,
        forward_yaw: QuantizedYaw,
        half_angle_deg: f64,
        max_distance: Fixed64,
        max_height: Fixed64,
        target: Vec3Fix,
    ) -> bool {
        if max_distance <= Fixed64::ZERO || half_angle_deg <= 0.0 {
            return false;
        }

        let diff = target - origin;

        // Vertical height cylinder bounds check
        if diff.y.abs() > max_height {
            return false;
        }

        // Horizontal squared distance check
        let dist_sq = (diff.x * diff.x) + (diff.z * diff.z);
        let max_dist_sq = max_distance * max_distance;
        if dist_sq > max_dist_sq {
            return false;
        }

        // Target at identical horizontal position is inside cone
        if dist_sq == Fixed64::ZERO {
            return true;
        }

        // Compute forward facing vector in horizontal plane (North = +Z, East = +X)
        let yaw_rad = forward_yaw.to_radians();
        let fwd_x = Fixed64::from_f64(yaw_rad.sin());
        let fwd_z = Fixed64::from_f64(yaw_rad.cos());

        // Horizontal dot product: (F . D)
        let dot = (fwd_x * diff.x) + (fwd_z * diff.z);
        if dot <= Fixed64::ZERO {
            return false; // Target is behind or perpendicular to caster
        }

        // Angle check: cos(theta) >= cos(half_angle)
        // (dot / |D|)^2 >= cos^2(half_angle)
        // dot^2 >= |D|^2 * cos^2(half_angle)
        let half_angle_clamped = half_angle_deg.clamp(0.0, 90.0);
        let cos_val = (half_angle_clamped.to_radians()).cos();
        let cos_sq = Fixed64::from_f64(cos_val * cos_val);

        let dot_sq = dot * dot;
        dot_sq >= dist_sq * cos_sq
    }

    /// Tests if a target point lies within a forward-projected oriented box (beam / rectangle).
    ///
    /// - `origin`: Starting base center of the box (caster position).
    /// - `forward_yaw`: Discrete heading direction.
    /// - `length`: Forward length of the box along the facing direction.
    /// - `width`: Total horizontal lateral width (extends width/2 left and width/2 right).
    /// - `height`: Total vertical height (extends height/2 up and height/2 down).
    /// - `target`: Evaluated entity position.
    pub fn test_box(
        origin: Vec3Fix,
        forward_yaw: QuantizedYaw,
        length: Fixed64,
        width: Fixed64,
        height: Fixed64,
        target: Vec3Fix,
    ) -> bool {
        if length <= Fixed64::ZERO || width <= Fixed64::ZERO || height <= Fixed64::ZERO {
            return false;
        }

        let diff = target - origin;

        // Vertical height check
        let half_height = height * Fixed64::from_f64(0.5);
        if diff.y.abs() > half_height {
            return false;
        }

        // Project into local forward and lateral coordinates
        let yaw_rad = forward_yaw.to_radians();
        let fwd_x = Fixed64::from_f64(yaw_rad.sin());
        let fwd_z = Fixed64::from_f64(yaw_rad.cos());

        // Forward coordinate along beam [0, length]
        let local_fwd = (fwd_x * diff.x) + (fwd_z * diff.z);
        if local_fwd < Fixed64::ZERO || local_fwd > length {
            return false;
        }

        // Lateral coordinate perpendicular to beam [-width/2, +width/2]
        // Left normal: (-fwd_z, fwd_x)
        let local_lat = (-fwd_z * diff.x) + (fwd_x * diff.z);
        let half_width = width * Fixed64::from_f64(0.5);
        if local_lat.abs() > half_width {
            return false;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sphere_collision() {
        let origin = Vec3Fix::from_f64(10.0, 0.0, 10.0);
        let radius = Fixed64::from_f64(5.0);

        // Point inside sphere
        let inside = Vec3Fix::from_f64(12.0, 2.0, 11.0);
        assert!(SpatialGeometry::test_sphere(origin, radius, inside));

        // Point on surface
        let surface = Vec3Fix::from_f64(15.0, 0.0, 10.0);
        assert!(SpatialGeometry::test_sphere(origin, radius, surface));

        // Point outside sphere
        let outside = Vec3Fix::from_f64(16.0, 0.0, 10.0);
        assert!(!SpatialGeometry::test_sphere(origin, radius, outside));
    }

    #[test]
    fn test_cone_collision() {
        let origin = Vec3Fix::from_f64(0.0, 0.0, 0.0);
        let facing_north = QuantizedYaw::NORTH; // Facing +Z
        let max_dist = Fixed64::from_f64(10.0);
        let max_height = Fixed64::from_f64(3.0);
        let half_angle = 45.0; // 90-degree total cone

        // Directly in front
        let in_front = Vec3Fix::from_f64(0.0, 0.0, 5.0);
        assert!(SpatialGeometry::test_cone(
            origin,
            facing_north,
            half_angle,
            max_dist,
            max_height,
            in_front
        ));

        // 30 degrees to the right (inside 45-degree half angle)
        let angled_in = Vec3Fix::from_f64(2.5, 0.0, 5.0);
        assert!(SpatialGeometry::test_cone(
            origin,
            facing_north,
            half_angle,
            max_dist,
            max_height,
            angled_in
        ));

        // 60 degrees to the right (outside 45-degree half angle)
        let angled_out = Vec3Fix::from_f64(8.0, 0.0, 2.0);
        assert!(!SpatialGeometry::test_cone(
            origin,
            facing_north,
            half_angle,
            max_dist,
            max_height,
            angled_out
        ));

        // Behind caster
        let behind = Vec3Fix::from_f64(0.0, 0.0, -5.0);
        assert!(!SpatialGeometry::test_cone(
            origin,
            facing_north,
            half_angle,
            max_dist,
            max_height,
            behind
        ));

        // Out of distance range
        let too_far = Vec3Fix::from_f64(0.0, 0.0, 15.0);
        assert!(!SpatialGeometry::test_cone(
            origin,
            facing_north,
            half_angle,
            max_dist,
            max_height,
            too_far
        ));

        // Too high vertically
        let too_high = Vec3Fix::from_f64(0.0, 5.0, 5.0);
        assert!(!SpatialGeometry::test_cone(
            origin,
            facing_north,
            half_angle,
            max_dist,
            max_height,
            too_high
        ));
    }

    #[test]
    fn test_box_collision() {
        let origin = Vec3Fix::from_f64(0.0, 0.0, 0.0);
        let facing_east = QuantizedYaw::EAST; // Facing +X
        let length = Fixed64::from_f64(10.0);
        let width = Fixed64::from_f64(4.0);
        let height = Fixed64::from_f64(2.0);

        // Point inside beam (5m ahead, 1m left)
        let inside = Vec3Fix::from_f64(5.0, 0.0, 1.0);
        assert!(SpatialGeometry::test_box(
            origin,
            facing_east,
            length,
            width,
            height,
            inside
        ));

        // Point too far forward
        let too_far = Vec3Fix::from_f64(12.0, 0.0, 0.0);
        assert!(!SpatialGeometry::test_box(
            origin,
            facing_east,
            length,
            width,
            height,
            too_far
        ));

        // Point too wide laterally (3m left, width is 4m so max is 2m)
        let too_wide = Vec3Fix::from_f64(5.0, 0.0, 3.0);
        assert!(!SpatialGeometry::test_box(
            origin,
            facing_east,
            length,
            width,
            height,
            too_wide
        ));

        // Point behind caster
        let behind = Vec3Fix::from_f64(-1.0, 0.0, 0.0);
        assert!(!SpatialGeometry::test_box(
            origin,
            facing_east,
            length,
            width,
            height,
            behind
        ));
    }
}
