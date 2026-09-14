//! Deterministic Replay and Flight Recording Primitives.
//!
//! Provides compact input frame logging, state keyframes, checksum calculation,
//! and divergence detection for server-side anti-cheat and competitive refereeing.

use crate::fixed::{Fixed64, Vec3Fix};
use crate::kinematics::{extrapolate, KinematicState};
use crate::quant::QuantizedYaw;

/// Maximum entities stored in a single keyframe snapshot.
pub const MAX_KEYFRAME_ENTITIES: usize = 16;

/// Compact record of a client input frame stamped with server tick.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedInputFrame {
    /// Authoritative server tick when this input was accepted.
    pub tick: u64,
    /// Entity ID that produced the input.
    pub entity_id: u32,
    /// Commanded velocity vector in 32.32 fixed-point meters/second.
    pub velocity: Vec3Fix,
    /// Commanded facing heading.
    pub yaw: QuantizedYaw,
    /// Movement flags (walking, sprinting, jumping).
    pub flags: u8,
}

impl Default for RecordedInputFrame {
    fn default() -> Self {
        Self {
            tick: 0,
            entity_id: 0,
            velocity: Vec3Fix::ZERO,
            yaw: QuantizedYaw::NORTH,
            flags: 0,
        }
    }
}

/// Instantaneous authoritative state snapshot of an entity at a keyframe tick.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityStateSnapshot {
    /// Entity ID.
    pub entity_id: u32,
    /// Authoritative global position in fixed-point meters.
    pub position: Vec3Fix,
    /// Velocity vector in fixed-point meters/second.
    pub velocity: Vec3Fix,
    /// Facing heading.
    pub yaw: QuantizedYaw,
    /// Movement state flags.
    pub flags: u8,
}

impl Default for EntityStateSnapshot {
    fn default() -> Self {
        Self {
            entity_id: 0,
            position: Vec3Fix::ZERO,
            velocity: Vec3Fix::ZERO,
            yaw: QuantizedYaw::NORTH,
            flags: 0,
        }
    }
}

impl EntityStateSnapshot {
    /// Converts snapshot into kinematic state for extrapolation.
    #[inline]
    pub fn to_kinematic_state(&self) -> KinematicState {
        KinematicState::with_velocity(self.position, self.velocity, self.yaw, self.flags)
    }
}

/// Full world state keyframe snapshot containing active entity transforms and checksum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyframeSnapshot {
    /// Server tick of this keyframe.
    pub tick: u64,
    /// Deterministic 32-bit Adler checksum over all active entity states.
    pub checksum: u32,
    /// Active entity states.
    pub entities: [Option<EntityStateSnapshot>; MAX_KEYFRAME_ENTITIES],
    /// Number of valid entities in this snapshot.
    pub entity_count: usize,
}

impl Default for KeyframeSnapshot {
    fn default() -> Self {
        Self {
            tick: 0,
            checksum: 0,
            entities: [const { None }; MAX_KEYFRAME_ENTITIES],
            entity_count: 0,
        }
    }
}

impl KeyframeSnapshot {
    /// Creates a new keyframe snapshot and computes its checksum.
    pub fn new(tick: u64, entities_slice: &[EntityStateSnapshot]) -> Self {
        let mut snapshot = Self {
            tick,
            checksum: 0,
            entities: [const { None }; MAX_KEYFRAME_ENTITIES],
            entity_count: entities_slice.len().min(MAX_KEYFRAME_ENTITIES),
        };

        for (i, &ent) in entities_slice
            .iter()
            .take(MAX_KEYFRAME_ENTITIES)
            .enumerate()
        {
            snapshot.entities[i] = Some(ent);
        }

        snapshot.checksum = compute_entities_checksum(&snapshot.entities[..snapshot.entity_count]);
        snapshot
    }

    /// Finds an entity snapshot by entity ID.
    pub fn get_entity(&self, entity_id: u32) -> Option<EntityStateSnapshot> {
        for ent in self.entities.iter().take(self.entity_count).flatten() {
            if ent.entity_id == entity_id {
                return Some(*ent);
            }
        }
        None
    }
}

/// Computes a deterministic Adler-32 checksum across entity state records.
pub fn compute_entities_checksum(slots: &[Option<EntityStateSnapshot>]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    const MOD_ADLER: u32 = 65521;

    for ent in slots.iter().flatten() {
        let id_bytes = ent.entity_id.to_le_bytes();
        let pos_bytes = [
            ent.position.x.raw().to_le_bytes(),
            ent.position.y.raw().to_le_bytes(),
            ent.position.z.raw().to_le_bytes(),
        ];
        let yaw_byte = ent.yaw.as_byte();
        let flags_byte = ent.flags;

        for byte in id_bytes {
            a = (a + byte as u32) % MOD_ADLER;
            b = (b + a) % MOD_ADLER;
        }
        for coord in pos_bytes {
            for byte in coord {
                a = (a + byte as u32) % MOD_ADLER;
                b = (b + a) % MOD_ADLER;
            }
        }
        a = (a + yaw_byte as u32) % MOD_ADLER;
        b = (b + a) % MOD_ADLER;
        a = (a + flags_byte as u32) % MOD_ADLER;
        b = (b + a) % MOD_ADLER;
    }

    (b << 16) | a
}

/// Divergence information between live simulation and recorded replay state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplayDivergence {
    /// Server tick where divergence was detected.
    pub tick: u64,
    /// Expected authoritative checksum from recording.
    pub expected_checksum: u32,
    /// Actual live simulation checksum.
    pub actual_checksum: u32,
    /// Divergent entity ID if identified.
    pub divergent_entity_id: Option<u32>,
    /// Measured spatial drift distance in meters.
    pub drift_distance: f64,
}

/// Playback speed multiplier for time-travel debugging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackSpeed {
    /// Slow-motion replay at quarter speed (0.25x).
    Quarter,
    /// Normal real-time replay speed (1.0x).
    Normal,
    /// Fast-forward replay at 4x speed.
    Fast,
    /// High-speed scanning at 16x speed.
    Ultra,
}

impl PlaybackSpeed {
    /// Returns the numerical multiplier as a float.
    pub fn multiplier(&self) -> f64 {
        match self {
            Self::Quarter => 0.25,
            Self::Normal => 1.0,
            Self::Fast => 4.0,
            Self::Ultra => 16.0,
        }
    }
}

/// Forward extrapolates an entity from a base state using an input frame.
pub fn extrapolate_with_input(
    base: &EntityStateSnapshot,
    input: &RecordedInputFrame,
    dt: Fixed64,
) -> EntityStateSnapshot {
    let initial_kin =
        KinematicState::with_velocity(base.position, input.velocity, input.yaw, input.flags);
    let next_kin = extrapolate(&initial_kin, 1, dt);

    EntityStateSnapshot {
        entity_id: base.entity_id,
        position: next_kin.position,
        velocity: input.velocity,
        yaw: input.yaw,
        flags: input.flags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checksum_determinism_and_sensitivity() {
        let ent1 = EntityStateSnapshot {
            entity_id: 1,
            position: Vec3Fix::new(Fixed64::from_i32(10), Fixed64::ZERO, Fixed64::from_i32(20)),
            velocity: Vec3Fix::ZERO,
            yaw: QuantizedYaw::NORTH,
            flags: 1,
        };
        let ent2 = EntityStateSnapshot {
            entity_id: 2,
            position: Vec3Fix::new(Fixed64::from_i32(30), Fixed64::ZERO, Fixed64::from_i32(40)),
            velocity: Vec3Fix::ZERO,
            yaw: QuantizedYaw::EAST,
            flags: 2,
        };

        let kf1 = KeyframeSnapshot::new(100, &[ent1, ent2]);
        let kf2 = KeyframeSnapshot::new(100, &[ent1, ent2]);
        assert_eq!(kf1.checksum, kf2.checksum);

        // Perturb position slightly
        let ent2_perturbed = EntityStateSnapshot {
            position: Vec3Fix::new(Fixed64::from_i32(31), Fixed64::ZERO, Fixed64::from_i32(40)),
            ..ent2
        };
        let kf3 = KeyframeSnapshot::new(100, &[ent1, ent2_perturbed]);
        assert_ne!(kf1.checksum, kf3.checksum);
    }

    #[test]
    fn test_playback_speed_multipliers() {
        assert_eq!(PlaybackSpeed::Quarter.multiplier(), 0.25);
        assert_eq!(PlaybackSpeed::Normal.multiplier(), 1.0);
        assert_eq!(PlaybackSpeed::Fast.multiplier(), 4.0);
        assert_eq!(PlaybackSpeed::Ultra.multiplier(), 16.0);
    }
}
