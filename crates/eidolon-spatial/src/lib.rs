//! High-density cache-aligned spatial hash grid and tiered Area of Interest (AoI) frequency manager.
//!
//! `eidolon-spatial` partitions entities into contiguous spatial buckets, enabling constant-time
//! insertion, removal, and distance-tiered replication scheduling.

#![deny(unsafe_code)]
#![warn(missing_docs)]

/// Spatial hash grid structures and cell coordinate hashing algorithms.
pub mod grid {
    /// 3D integer coordinate identifying a discrete spatial cell bucket.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct CellCoord {
        /// Cell index on horizontal X axis.
        pub x: i32,
        /// Cell index on vertical Y axis.
        pub y: i32,
        /// Cell index on horizontal Z axis.
        pub z: i32,
    }

    impl CellCoord {
        /// Creates a new cell coordinate.
        #[inline]
        pub const fn new(x: i32, y: i32, z: i32) -> Self {
            Self { x, y, z }
        }
    }
}

/// Dynamic Area of Interest (AoI) spatial frequency tiers.
pub mod tier {
    /// Update frequency tiers assigned to entities based on observer distance.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub enum FrequencyTier {
        /// Immediate Tier (< 10 meters): High-frequency 10 Hz updates.
        Immediate = 0,
        /// Mid Tier (10m to 50 meters): Interpolated 2 Hz updates.
        Mid = 1,
        /// Horizon Tier (> 50 meters): Event-only state change updates.
        Horizon = 2,
    }

    impl FrequencyTier {
        /// Returns the target update interval in simulation ticks (assuming 20 Hz / 50ms server ticks).
        #[inline]
        pub const fn tick_interval(self) -> u32 {
            match self {
                Self::Immediate => 2, // Every 2 ticks (100ms = 10 Hz)
                Self::Mid => 10,      // Every 10 ticks (500ms = 2 Hz)
                Self::Horizon => 0,   // State change event driven only
            }
        }
    }
}

/// Visibility queries and observer filtering.
pub mod aoi {
    use crate::tier::FrequencyTier;

    /// Evaluates distance squared against tier thresholds to categorize frequency.
    #[inline]
    pub fn calculate_tier_from_dist_sq(dist_sq: u64) -> FrequencyTier {
        // 10m squared = 100m^2
        const IMMEDIATE_THRESHOLD_SQ: u64 = 100;
        // 50m squared = 2500m^2
        const MID_THRESHOLD_SQ: u64 = 2500;

        if dist_sq <= IMMEDIATE_THRESHOLD_SQ {
            FrequencyTier::Immediate
        } else if dist_sq <= MID_THRESHOLD_SQ {
            FrequencyTier::Mid
        } else {
            FrequencyTier::Horizon
        }
    }
}

#[cfg(test)]
mod tests {
    use super::aoi::calculate_tier_from_dist_sq;
    use super::grid::CellCoord;
    use super::tier::FrequencyTier;

    #[test]
    fn test_cell_coord_new() {
        let coord = CellCoord::new(1, -2, 3);
        assert_eq!(coord.x, 1);
        assert_eq!(coord.y, -2);
        assert_eq!(coord.z, 3);
    }

    #[test]
    fn test_frequency_tier_intervals() {
        assert_eq!(FrequencyTier::Immediate.tick_interval(), 2);
        assert_eq!(FrequencyTier::Mid.tick_interval(), 10);
        assert_eq!(FrequencyTier::Horizon.tick_interval(), 0);
    }

    #[test]
    fn test_tier_calculation() {
        assert_eq!(calculate_tier_from_dist_sq(25), FrequencyTier::Immediate);
        assert_eq!(calculate_tier_from_dist_sq(100), FrequencyTier::Immediate);
        assert_eq!(calculate_tier_from_dist_sq(101), FrequencyTier::Mid);
        assert_eq!(calculate_tier_from_dist_sq(2500), FrequencyTier::Mid);
        assert_eq!(calculate_tier_from_dist_sq(2501), FrequencyTier::Horizon);
    }
}
