//! Sensory perception models for autonomous NPCs.
//!
//! Provides sight cone field-of-view evaluation, acoustic detection,
//! and stealth rating checks with zero heap allocations.

use core::f32::consts::PI;

/// Configurable sensory profile defining an NPC's perceptual capabilities.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensoryProfile {
    /// Maximum visual range in meters.
    pub sight_range: f32,
    /// Cosine of the half-angle of the sight field of view (e.g. cos(60 deg) = 0.5 for 120 deg cone).
    pub sight_half_angle_cos: f32,
    /// Maximum omnidirectional acoustic hearing radius in meters.
    pub hearing_range: f32,
    /// Maximum vertical perception height delta in meters.
    pub vertical_range: f32,
    /// Stealth detection rating (target stealth rating must be <= this value to be detected).
    pub stealth_detection: u32,
}

impl Default for SensoryProfile {
    fn default() -> Self {
        Self {
            sight_range: 30.0,
            sight_half_angle_cos: 0.5, // 120 degree total vision cone (60 degrees to each side)
            hearing_range: 15.0,
            vertical_range: 10.0,
            stealth_detection: 100,
        }
    }
}

impl SensoryProfile {
    /// Creates a new sensory profile with explicit field-of-view angle in degrees.
    pub fn new(
        sight_range: f32,
        fov_degrees: f32,
        hearing_range: f32,
        stealth_detection: u32,
    ) -> Self {
        let half_angle_rad = (fov_degrees.clamp(0.0, 360.0) * 0.5) * (PI / 180.0);
        Self {
            sight_range: sight_range.max(0.0),
            sight_half_angle_cos: half_angle_rad.cos(),
            hearing_range: hearing_range.max(0.0),
            vertical_range: 10.0,
            stealth_detection,
        }
    }
}

/// Result of evaluating an observer's perception of a target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerceptionResult {
    /// Whether the target was perceived via any sensory channel.
    pub perceived: bool,
    /// Whether the target was perceived visually inside the field-of-view cone.
    pub is_visual: bool,
    /// Whether the target was perceived acoustically via proximity noise.
    pub is_acoustic: bool,
    /// Horizontal Euclidean distance to the target in meters.
    pub distance: f32,
}

impl PerceptionResult {
    /// Constructs a non-perceived default result.
    pub const fn none() -> Self {
        Self {
            perceived: false,
            is_visual: false,
            is_acoustic: false,
            distance: 0.0,
        }
    }
}

/// Evaluates whether an observer perceives a target entity.
///
/// Operates with zero heap allocations using standard vector geometry.
#[inline]
pub fn evaluate_perception(
    observer_pos: (f32, f32, f32),
    observer_yaw_deg: f32,
    target_pos: (f32, f32, f32),
    target_stealth: u32,
    profile: &SensoryProfile,
) -> PerceptionResult {
    let dx = target_pos.0 - observer_pos.0;
    let dy = target_pos.1 - observer_pos.1;
    let dz = target_pos.2 - observer_pos.2;

    if dy.abs() > profile.vertical_range {
        return PerceptionResult::none();
    }

    let dist_sq = dx * dx + dz * dz;
    let max_range = profile.sight_range.max(profile.hearing_range);
    if dist_sq > max_range * max_range {
        return PerceptionResult::none();
    }

    let distance = dist_sq.sqrt();

    // Stealth check: if target stealth rating exceeds observer detection, target is undetected
    if target_stealth > profile.stealth_detection {
        return PerceptionResult {
            perceived: false,
            is_visual: false,
            is_acoustic: false,
            distance,
        };
    }

    // Acoustic check (omnidirectional within hearing range)
    let is_acoustic = distance <= profile.hearing_range;

    // Visual check (requires distance <= sight_range AND inside FOV cone)
    let is_visual = if distance <= profile.sight_range && distance > 0.001 {
        let yaw_rad = observer_yaw_deg * (PI / 180.0);
        let fwd_x = yaw_rad.sin();
        let fwd_z = yaw_rad.cos();

        let to_target_x = dx / distance;
        let to_target_z = dz / distance;

        let dot = fwd_x * to_target_x + fwd_z * to_target_z;
        dot >= profile.sight_half_angle_cos
    } else {
        false
    };

    PerceptionResult {
        perceived: is_visual || is_acoustic,
        is_visual,
        is_acoustic,
        distance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direct_front_visual_perception() {
        let profile = SensoryProfile::new(30.0, 120.0, 10.0, 100);
        let observer_pos = (0.0, 0.0, 0.0);
        let observer_yaw = 0.0; // Facing +Z
        let target_pos = (0.0, 0.0, 20.0); // Directly in front at 20m

        let result = evaluate_perception(observer_pos, observer_yaw, target_pos, 50, &profile);
        assert!(result.perceived);
        assert!(result.is_visual);
        assert!(!result.is_acoustic); // Outside 10m hearing range
        assert!((result.distance - 20.0).abs() < 0.001);
    }

    #[test]
    fn test_behind_observer_acoustic_only() {
        let profile = SensoryProfile::new(30.0, 120.0, 10.0, 100);
        let observer_pos = (0.0, 0.0, 0.0);
        let observer_yaw = 0.0; // Facing +Z
        let target_pos = (0.0, 0.0, -8.0); // Directly behind at 8m (within 10m hearing)

        let result = evaluate_perception(observer_pos, observer_yaw, target_pos, 50, &profile);
        assert!(result.perceived);
        assert!(!result.is_visual); // Behind observer: outside 120 degree cone
        assert!(result.is_acoustic);
    }

    #[test]
    fn test_outside_fov_cone_and_outside_hearing() {
        let profile = SensoryProfile::new(30.0, 90.0, 5.0, 100); // 90 degree FOV (45 deg half-angle)
        let observer_pos = (0.0, 0.0, 0.0);
        let observer_yaw = 0.0; // Facing +Z
        let target_pos = (20.0, 0.0, 0.0); // Directly to the right at 90 degrees, 20m away

        let result = evaluate_perception(observer_pos, observer_yaw, target_pos, 50, &profile);
        assert!(!result.perceived);
        assert!(!result.is_visual);
        assert!(!result.is_acoustic);
    }

    #[test]
    fn test_stealth_concealment() {
        let profile = SensoryProfile::new(30.0, 120.0, 10.0, 50); // Detection rating 50
        let observer_pos = (0.0, 0.0, 0.0);
        let observer_yaw = 0.0;
        let target_pos = (0.0, 0.0, 5.0); // Directly in front at 5m

        // Target with stealth 80 (exceeds detection 50)
        let result = evaluate_perception(observer_pos, observer_yaw, target_pos, 80, &profile);
        assert!(!result.perceived);
    }
}
