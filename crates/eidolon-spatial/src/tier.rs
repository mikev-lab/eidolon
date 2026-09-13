//! Dynamic Area of Interest (AoI) spatial frequency tiers and hysteresis state machine.
//!
//! Categorizes replication frequency by observer distance and enforces spatial deadbands
//! to eliminate tier flickering when entities loiter near threshold boundaries.

use eidolon_core::fixed::Fixed64;

/// Squared distance threshold for promoting an entity to the Immediate tier (9.0m squared = 81 m^2).
pub const IMMEDIATE_PROMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(81);

/// Squared distance threshold for demoting an entity from the Immediate tier (11.0m squared = 121 m^2).
pub const IMMEDIATE_DEMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(121);

/// Squared distance threshold for promoting an entity to the Mid tier (48.0m squared = 2304 m^2).
pub const MID_PROMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(2304);

/// Squared distance threshold for demoting an entity from the Mid tier (52.0m squared = 2704 m^2).
pub const MID_DEMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(2704);

/// Update frequency tiers assigned to entities based on observer distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum FrequencyTier {
    /// Immediate Tier (< 10 meters): High-frequency 10 Hz updates.
    #[default]
    Immediate = 0,
    /// Mid Tier (10m to 50 meters): Interpolated 2 Hz updates.
    Mid = 1,
    /// Horizon Tier (> 50 meters): Event-only state change updates.
    Horizon = 2,
}

impl FrequencyTier {
    /// Target update interval in simulation ticks (assuming 20 Hz / 50ms server ticks).
    #[inline]
    pub const fn tick_interval(self) -> u32 {
        match self {
            Self::Immediate => 2, // Every 2 ticks (100ms = 10 Hz)
            Self::Mid => 10,      // Every 10 ticks (500ms = 2 Hz)
            Self::Horizon => 0,   // State change event driven only
        }
    }

    /// Evaluates whether an entity in this tier should be dispatched in the given tick.
    #[inline]
    pub fn should_dispatch(self, tick_index: u64, is_state_changed: bool) -> bool {
        if is_state_changed {
            return true;
        }

        match self {
            Self::Immediate => tick_index.is_multiple_of(2),
            Self::Mid => tick_index.is_multiple_of(10),
            Self::Horizon => false,
        }
    }

    /// Classifies an entity into an initial frequency tier from squared distance.
    #[inline]
    pub fn classify_initial(dist_sq: Fixed64) -> Self {
        // 10m squared = 100
        const TEN_METERS_SQ: Fixed64 = Fixed64::from_i32(100);
        // 50m squared = 2500
        const FIFTY_METERS_SQ: Fixed64 = Fixed64::from_i32(2500);

        if dist_sq <= TEN_METERS_SQ {
            Self::Immediate
        } else if dist_sq <= FIFTY_METERS_SQ {
            Self::Mid
        } else {
            Self::Horizon
        }
    }

    /// Updates the frequency tier using spatial hysteresis to eliminate boundary flickering.
    pub fn update_with_hysteresis(self, dist_sq: Fixed64) -> Self {
        match self {
            Self::Immediate => {
                if dist_sq > MID_DEMOTION_DIST_SQ {
                    Self::Horizon
                } else if dist_sq > IMMEDIATE_DEMOTION_DIST_SQ {
                    Self::Mid
                } else {
                    Self::Immediate
                }
            }
            Self::Mid => {
                if dist_sq < IMMEDIATE_PROMOTION_DIST_SQ {
                    Self::Immediate
                } else if dist_sq > MID_DEMOTION_DIST_SQ {
                    Self::Horizon
                } else {
                    Self::Mid
                }
            }
            Self::Horizon => {
                if dist_sq < IMMEDIATE_PROMOTION_DIST_SQ {
                    Self::Immediate
                } else if dist_sq < MID_PROMOTION_DIST_SQ {
                    Self::Mid
                } else {
                    Self::Horizon
                }
            }
        }
    }
}
