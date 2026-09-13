//! Dynamic Area of Interest (AoI) spatial frequency tiers and hysteresis state machine.
//!
//! Categorizes replication frequency by observer distance and enforces spatial deadbands
//! to eliminate tier flickering when entities loiter near threshold boundaries.
//! Supports both standard 3-tier and high-density continuous 5-tier radial decay modes.

use eidolon_core::fixed::Fixed64;

/// Squared distance threshold for promoting an entity to the Immediate tier in 3-tier mode (9.0m squared = 81 m^2).
pub const IMMEDIATE_PROMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(81);

/// Squared distance threshold for demoting an entity from the Immediate tier in 3-tier mode (11.0m squared = 121 m^2).
pub const IMMEDIATE_DEMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(121);

/// Squared distance threshold for promoting an entity to the Mid tier in 3-tier mode (48.0m squared = 2304 m^2).
pub const MID_PROMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(2304);

/// Squared distance threshold for demoting an entity from the Mid tier in 3-tier mode (52.0m squared = 2704 m^2).
pub const MID_DEMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(2704);

/// Squared distance threshold for promoting an entity to 5-tier Immediate (< 14m squared = 196 m^2).
pub const TIER5_IMMEDIATE_PROMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(196);

/// Squared distance threshold for demoting an entity from 5-tier Immediate (> 16m squared = 256 m^2).
pub const TIER5_IMMEDIATE_DEMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(256);

/// Squared distance threshold for promoting an entity to 5-tier Tactical (< 38m squared = 1444 m^2).
pub const TIER5_TACTICAL_PROMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(1444);

/// Squared distance threshold for demoting an entity from 5-tier Tactical (> 42m squared = 1764 m^2).
pub const TIER5_TACTICAL_DEMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(1764);

/// Squared distance threshold for promoting an entity to 5-tier Midfield (< 95m squared = 9025 m^2).
pub const TIER5_MIDFIELD_PROMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(9025);

/// Squared distance threshold for demoting an entity from 5-tier Midfield (> 105m squared = 11025 m^2).
pub const TIER5_MIDFIELD_DEMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(11025);

/// Squared distance threshold for promoting an entity to 5-tier Horizon (< 290m squared = 84100 m^2).
pub const TIER5_HORIZON_PROMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(84100);

/// Squared distance threshold for demoting an entity from 5-tier Horizon (> 310m squared = 96100 m^2).
pub const TIER5_HORIZON_DEMOTION_DIST_SQ: Fixed64 = Fixed64::from_i32(96100);

/// Spatial tier classification model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TierModel {
    /// Standard 3-tier spatial model (Immediate <10m, Mid 10m-50m, Horizon >50m).
    #[default]
    Standard3Tier,
    /// Continuous 5-tier radial decay model for extreme density (Immediate, Tactical, Midfield, Horizon, Macro).
    Continuous5Tier,
}

/// Update frequency tiers assigned to entities based on observer distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(u8)]
pub enum FrequencyTier {
    /// Tier 0: Immediate Melee (< 15m / < 10m in 3-tier): High-frequency 10-20 Hz updates. Full 7-byte transform.
    #[default]
    Immediate = 0,
    /// Tier 1: Tactical Combat (15m to 40m): High-priority 10 Hz updates. Full 7-byte transform.
    Tactical = 1,
    /// Tier 2: Midfield / Mid (40m to 100m / 10m to 50m in 3-tier): 2-4 Hz updates. 5-byte delta transform.
    Mid = 2,
    /// Tier 3: Horizon (100m to 300m / > 50m in 3-tier): 1 Hz updates or state-change events. 4-byte delta transform.
    Horizon = 3,
    /// Tier 4: Macro Overview (> 300m): Strategic 0.2 Hz updates or beacons. 2-byte compact transform.
    Macro = 4,
}

impl FrequencyTier {
    /// Alias for Midfield tier.
    pub const MIDFIELD: Self = Self::Mid;

    /// Backward-compatible alias for Midfield tier in PascalCase.
    #[allow(non_upper_case_globals)]
    pub const Midfield: Self = Self::Mid;

    /// Target update interval in simulation ticks (assuming 20 Hz / 50ms server ticks).
    #[inline]
    pub const fn tick_interval(self) -> u32 {
        match self {
            Self::Immediate => 2, // Every 2 ticks (100ms = 10 Hz)
            Self::Tactical => 2,  // Every 2 ticks (100ms = 10 Hz)
            Self::Mid => 10,      // Every 10 ticks (500ms = 2 Hz)
            Self::Horizon => 20,  // Every 20 ticks (1000ms = 1 Hz)
            Self::Macro => 100,   // Every 100 ticks (5000ms = 0.2 Hz)
        }
    }

    /// Returns the wire payload size in bytes for an entity transform update in this tier.
    #[inline]
    pub const fn payload_size_bytes(self) -> usize {
        match self {
            Self::Immediate => 7,
            Self::Tactical => 7,
            Self::Mid => 5,
            Self::Horizon => 4,
            Self::Macro => 2,
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
            Self::Tactical => tick_index.is_multiple_of(2),
            Self::Mid => tick_index.is_multiple_of(10),
            Self::Horizon => tick_index.is_multiple_of(20),
            Self::Macro => tick_index.is_multiple_of(100),
        }
    }

    /// Classifies an entity into an initial frequency tier from squared distance using the standard 3-tier model.
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

    /// Classifies an entity into an initial frequency tier from squared distance using the continuous 5-tier model.
    #[inline]
    pub fn classify_5tier(dist_sq: Fixed64) -> Self {
        // 15m squared = 225
        const FIFTEEN_METERS_SQ: Fixed64 = Fixed64::from_i32(225);
        // 40m squared = 1600
        const FORTY_METERS_SQ: Fixed64 = Fixed64::from_i32(1600);
        // 100m squared = 10000
        const HUNDRED_METERS_SQ: Fixed64 = Fixed64::from_i32(10000);
        // 300m squared = 90000
        const THREE_HUNDRED_METERS_SQ: Fixed64 = Fixed64::from_i32(90000);

        if dist_sq <= FIFTEEN_METERS_SQ {
            Self::Immediate
        } else if dist_sq <= FORTY_METERS_SQ {
            Self::Tactical
        } else if dist_sq <= HUNDRED_METERS_SQ {
            Self::Mid
        } else if dist_sq <= THREE_HUNDRED_METERS_SQ {
            Self::Horizon
        } else {
            Self::Macro
        }
    }

    /// Updates the frequency tier using spatial hysteresis in the standard 3-tier model.
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
            Self::Tactical | Self::Mid => {
                if dist_sq < IMMEDIATE_PROMOTION_DIST_SQ {
                    Self::Immediate
                } else if dist_sq > MID_DEMOTION_DIST_SQ {
                    Self::Horizon
                } else {
                    Self::Mid
                }
            }
            Self::Horizon | Self::Macro => {
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

    /// Updates the frequency tier using spatial hysteresis in the continuous 5-tier model.
    pub fn update_5tier_with_hysteresis(self, dist_sq: Fixed64) -> Self {
        match self {
            Self::Immediate => {
                if dist_sq > TIER5_HORIZON_DEMOTION_DIST_SQ {
                    Self::Macro
                } else if dist_sq > TIER5_MIDFIELD_DEMOTION_DIST_SQ {
                    Self::Horizon
                } else if dist_sq > TIER5_TACTICAL_DEMOTION_DIST_SQ {
                    Self::Mid
                } else if dist_sq > TIER5_IMMEDIATE_DEMOTION_DIST_SQ {
                    Self::Tactical
                } else {
                    Self::Immediate
                }
            }
            Self::Tactical => {
                if dist_sq < TIER5_IMMEDIATE_PROMOTION_DIST_SQ {
                    Self::Immediate
                } else if dist_sq > TIER5_HORIZON_DEMOTION_DIST_SQ {
                    Self::Macro
                } else if dist_sq > TIER5_MIDFIELD_DEMOTION_DIST_SQ {
                    Self::Horizon
                } else if dist_sq > TIER5_TACTICAL_DEMOTION_DIST_SQ {
                    Self::Mid
                } else {
                    Self::Tactical
                }
            }
            Self::Mid => {
                if dist_sq < TIER5_IMMEDIATE_PROMOTION_DIST_SQ {
                    Self::Immediate
                } else if dist_sq < TIER5_TACTICAL_PROMOTION_DIST_SQ {
                    Self::Tactical
                } else if dist_sq > TIER5_HORIZON_DEMOTION_DIST_SQ {
                    Self::Macro
                } else if dist_sq > TIER5_MIDFIELD_DEMOTION_DIST_SQ {
                    Self::Horizon
                } else {
                    Self::Mid
                }
            }
            Self::Horizon => {
                if dist_sq < TIER5_IMMEDIATE_PROMOTION_DIST_SQ {
                    Self::Immediate
                } else if dist_sq < TIER5_TACTICAL_PROMOTION_DIST_SQ {
                    Self::Tactical
                } else if dist_sq < TIER5_MIDFIELD_PROMOTION_DIST_SQ {
                    Self::Mid
                } else if dist_sq > TIER5_HORIZON_DEMOTION_DIST_SQ {
                    Self::Macro
                } else {
                    Self::Horizon
                }
            }
            Self::Macro => {
                if dist_sq < TIER5_IMMEDIATE_PROMOTION_DIST_SQ {
                    Self::Immediate
                } else if dist_sq < TIER5_TACTICAL_PROMOTION_DIST_SQ {
                    Self::Tactical
                } else if dist_sq < TIER5_MIDFIELD_PROMOTION_DIST_SQ {
                    Self::Mid
                } else if dist_sq < TIER5_HORIZON_PROMOTION_DIST_SQ {
                    Self::Horizon
                } else {
                    Self::Macro
                }
            }
        }
    }
}
