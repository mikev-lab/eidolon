//! Server-authoritative abilities, cast progress state machines, and cooldown tracking.
//!
//! Provides deterministic spell casting, interrupt mechanics on movement or damage,
//! and geometric area-of-effect validation.

use eidolon_core::fixed::{Fixed64, Vec3Fix};

/// Geometric target shape of an ability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AbilityShape {
    /// Targeted directly at an entity within range.
    SingleTarget {
        /// Maximum allowed casting distance.
        max_range: Fixed64,
    },
    /// Radial spherical explosion centered on caster or target point.
    RadiusSphere {
        /// Explosion radius in meters.
        radius: Fixed64,
    },
    /// Forward-facing circular cone originating from caster heading.
    ForwardCone {
        /// Half of the cone angle in degrees (e.g. 45.0 for 90-degree spread).
        half_angle_deg: f64,
        /// Maximum range distance.
        max_distance: Fixed64,
        /// Maximum vertical extent above/below caster.
        max_height: Fixed64,
    },
    /// Forward-facing rectangular beam / box.
    ForwardBox {
        /// Forward length.
        length: Fixed64,
        /// Total lateral width.
        width: Fixed64,
        /// Total vertical height.
        height: Fixed64,
    },
}

/// Static blueprint specification of a character ability.
#[derive(Debug, Clone, PartialEq)]
pub struct AbilityDefinition {
    /// Unique ability identifier.
    pub ability_id: u32,
    /// Display name.
    pub name: &'static str,
    /// Cast duration in simulation ticks (e.g. 20 ticks = 1.0s at 20 Hz). 0 = Instant.
    pub cast_duration_ticks: u32,
    /// Recovery cooldown duration in simulation ticks.
    pub cooldown_ticks: u32,
    /// Resource cost consumed on cast (e.g. Mana / Energy).
    pub resource_cost: u32,
    /// Base damage points dealt to hostile targets.
    pub base_damage: u32,
    /// Base healing points granted to friendly targets.
    pub base_heal: u32,
    /// Geometric targeting shape.
    pub shape: AbilityShape,
}

/// Returns the blueprint definition for standard built-in abilities.
pub fn get_ability_definition(ability_id: u32) -> Option<AbilityDefinition> {
    match ability_id {
        // Fireball: 1.0s cast, 25 mana, 2.0s cooldown, 20m range, 60 damage
        1 => Some(AbilityDefinition {
            ability_id: 1,
            name: "Fireball",
            cast_duration_ticks: 20,
            cooldown_ticks: 40,
            resource_cost: 25,
            base_damage: 60,
            base_heal: 0,
            shape: AbilityShape::SingleTarget {
                max_range: Fixed64::from_f64(20.0),
            },
        }),

        // Arcane Cleave: Instant, 15 mana, 1.5s cooldown, 45-deg cone (8m), 40 damage
        2 => Some(AbilityDefinition {
            ability_id: 2,
            name: "Arcane Cleave",
            cast_duration_ticks: 0,
            cooldown_ticks: 30,
            resource_cost: 15,
            base_damage: 40,
            base_heal: 0,
            shape: AbilityShape::ForwardCone {
                half_angle_deg: 45.0,
                max_distance: Fixed64::from_f64(8.0),
                max_height: Fixed64::from_f64(3.0),
            },
        }),

        // Healing Light: 1.5s cast, 30 mana, 3.0s cooldown, 25m range, 80 heal
        3 => Some(AbilityDefinition {
            ability_id: 3,
            name: "Healing Light",
            cast_duration_ticks: 30,
            cooldown_ticks: 60,
            resource_cost: 30,
            base_damage: 0,
            base_heal: 80,
            shape: AbilityShape::SingleTarget {
                max_range: Fixed64::from_f64(25.0),
            },
        }),

        // Frost Nova: Instant, 35 mana, 5.0s cooldown, 10m radial sphere, 35 damage
        4 => Some(AbilityDefinition {
            ability_id: 4,
            name: "Frost Nova",
            cast_duration_ticks: 0,
            cooldown_ticks: 100,
            resource_cost: 35,
            base_damage: 35,
            base_heal: 0,
            shape: AbilityShape::RadiusSphere {
                radius: Fixed64::from_f64(10.0),
            },
        }),

        _ => None,
    }
}

/// Reason explaining why an active cast was terminated prematurely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastInterruptedReason {
    /// Caster moved beyond allowed stationary threshold.
    Movement,
    /// Caster took combat damage during non-unbreakable cast.
    Damage,
    /// Player explicitly issued cancel command.
    Cancelled,
    /// Target entity despawned or moved out of range.
    TargetLost,
}

/// State machine tracking an active character ability cast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CastState {
    /// Player is idle and able to initiate a new cast.
    #[default]
    Idle,
    /// Player is progressing an authoritative cast bar.
    Casting {
        /// Blueprint ability identifier.
        ability_id: u32,
        /// Optional target entity identifier.
        target_id: u32,
        /// Simulation tick when the cast began.
        start_tick: u64,
        /// Required duration in ticks to complete.
        duration_ticks: u32,
        /// Initial caster position when cast started (for movement break checks).
        start_pos: Vec3Fix,
    },
}

impl CastState {
    /// Starts a new ability cast.
    pub fn start_cast(
        ability_id: u32,
        target_id: u32,
        current_tick: u64,
        duration_ticks: u32,
        start_pos: Vec3Fix,
    ) -> Self {
        Self::Casting {
            ability_id,
            target_id,
            start_tick: current_tick,
            duration_ticks,
            start_pos,
        }
    }

    /// Checks whether the active cast has reached completion at `current_tick`.
    ///
    /// Returns `Some(ability_id, target_id)` if finished, or `None` if still casting or idle.
    pub fn check_completion(&self, current_tick: u64) -> Option<(u32, u32)> {
        if let Self::Casting {
            ability_id,
            target_id,
            start_tick,
            duration_ticks,
            ..
        } = *self
        {
            if current_tick >= start_tick + duration_ticks as u64 {
                return Some((ability_id, target_id));
            }
        }
        None
    }

    /// Validates if movement broke the cast (distance > 0.5m from start_pos).
    pub fn check_movement_interrupt(&self, current_pos: Vec3Fix) -> bool {
        if let Self::Casting { start_pos, .. } = *self {
            let dist_sq = (current_pos - start_pos).magnitude_squared();
            // 0.5m threshold squared = 0.25 m^2
            dist_sq > Fixed64::from_f64(0.25)
        } else {
            false
        }
    }

    /// Cancels active cast, resetting state to `Idle`.
    pub fn cancel(&mut self) {
        *self = Self::Idle;
    }
}

/// Maximum concurrent active ability cooldown timers tracked per entity.
pub const MAX_COOLDOWNS_PER_ENTITY: usize = 16;

/// Zero-allocation registry tracking recovery cooldown timers per ability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CooldownTracker {
    /// Fixed-capacity array of active (ability_id, ready_at_tick) pairs.
    entries: [Option<(u32, u64)>; MAX_COOLDOWNS_PER_ENTITY],
}

impl CooldownTracker {
    /// Constructs an empty cooldown tracker with zero dynamic heap allocations.
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_COOLDOWNS_PER_ENTITY],
        }
    }

    /// Checks if an ability is ready to cast at `current_tick`.
    #[inline]
    pub fn is_ready(&self, ability_id: u32, current_tick: u64) -> bool {
        for entry in self.entries.iter().flatten() {
            if entry.0 == ability_id {
                return current_tick >= entry.1;
            }
        }
        true
    }

    /// Triggers a cooldown for the specified ability lasting `duration_ticks`.
    pub fn trigger(&mut self, ability_id: u32, current_tick: u64, duration_ticks: u32) {
        let ready_at = current_tick + duration_ticks as u64;

        // 1. If ability is already in table, update ready_at
        for entry in self.entries.iter_mut().flatten() {
            if entry.0 == ability_id {
                entry.1 = ready_at;
                return;
            }
        }

        // 2. Find empty slot
        for slot in self.entries.iter_mut() {
            if slot.is_none() {
                *slot = Some((ability_id, ready_at));
                return;
            }
        }

        // 3. Reclaim an expired slot if full
        for slot in self.entries.iter_mut() {
            if let Some(entry) = slot {
                if current_tick >= entry.1 {
                    *slot = Some((ability_id, ready_at));
                    return;
                }
            }
        }

        // 4. Overwrite oldest expiration if table completely saturated with active cooldowns
        if let Some(first) = self.entries.get_mut(0) {
            *first = Some((ability_id, ready_at));
        }
    }

    /// Returns remaining ticks until ability is ready, or 0 if available.
    pub fn remaining_ticks(&self, ability_id: u32, current_tick: u64) -> u32 {
        for entry in self.entries.iter().flatten() {
            if entry.0 == ability_id {
                return if entry.1 > current_tick {
                    (entry.1 - current_tick) as u32
                } else {
                    0
                };
            }
        }
        0
    }
}

impl Default for CooldownTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cast_progression_and_completion() {
        let start_pos = Vec3Fix::from_f64(100.0, 0.0, 100.0);
        let mut cast = CastState::start_cast(1, 101, 100, 20, start_pos);

        // Before completion tick
        assert_eq!(cast.check_completion(110), None);
        assert_eq!(cast.check_completion(119), None);

        // Exactly at completion tick
        assert_eq!(cast.check_completion(120), Some((1, 101)));
        // Past completion tick
        assert_eq!(cast.check_completion(125), Some((1, 101)));

        cast.cancel();
        assert_eq!(cast, CastState::Idle);
    }

    #[test]
    fn test_movement_interrupt() {
        let start_pos = Vec3Fix::from_f64(100.0, 0.0, 100.0);
        let cast = CastState::start_cast(1, 101, 100, 20, start_pos);

        // Micro-jitter under 0.5m (e.g. 0.2m) does not interrupt
        let slight_move = Vec3Fix::from_f64(100.2, 0.0, 100.0);
        assert!(!cast.check_movement_interrupt(slight_move));

        // Substantial move > 0.5m (e.g. 1.0m) interrupts
        let large_move = Vec3Fix::from_f64(101.0, 0.0, 100.0);
        assert!(cast.check_movement_interrupt(large_move));
    }

    #[test]
    fn test_cooldown_tracker() {
        let mut tracker = CooldownTracker::new();
        assert!(tracker.is_ready(1, 100));

        tracker.trigger(1, 100, 40);
        assert!(!tracker.is_ready(1, 100));
        assert_eq!(tracker.remaining_ticks(1, 100), 40);

        assert!(!tracker.is_ready(1, 139));
        assert_eq!(tracker.remaining_ticks(1, 139), 1);

        assert!(tracker.is_ready(1, 140));
        assert_eq!(tracker.remaining_ticks(1, 140), 0);

        // Saturation test: fill all 16 slots with active cooldowns
        for id in 2..=17 {
            tracker.trigger(id, 100, 50);
            assert!(!tracker.is_ready(id, 100));
        }
        // Slot recycling
        tracker.trigger(999, 200, 10);
        assert!(!tracker.is_ready(999, 200));
    }
}
