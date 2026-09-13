//! Visibility queries, observer interest management, and adaptive load shedding.
//!
//! Orchestrates dynamic multi-tier entity replication across simulation ticks,
//! shedding non-essential updates under CPU overruns to preserve simulation stability.

use eidolon_core::fixed::{Fixed64, Vec3Fix};

use crate::grid::SpatialHashGrid;
use crate::tier::FrequencyTier;

/// Maximum horizontal Area of Interest radius in meters (50 meters).
pub const MAX_AOI_RADIUS_METERS: f64 = 50.0;

/// Squared maximum horizontal Area of Interest radius: 50^2 = 2500 m^2.
pub const MAX_AOI_RADIUS_SQ: Fixed64 = Fixed64::from_i32(2500);

/// Adaptive load shedding levels activated when simulation ticks exceed time budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum LoadSheddingLevel {
    /// Normal operation: Immediate 10 Hz, Mid 2 Hz, Horizon event-driven.
    #[default]
    None = 0,
    /// Level 1 load shedding: Throttles Mid tier to 1 Hz, drops Horizon events.
    Level1 = 1,
    /// Level 2 load shedding: Drops Mid and Horizon tiers entirely, protecting Immediate 10 Hz combat.
    Level2 = 2,
}

/// Area of Interest replication scheduler and load manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AoIScheduler {
    load_shedding: LoadSheddingLevel,
}

impl AoIScheduler {
    /// Constructs a new AoI scheduler with default scheduling parameters.
    #[inline]
    pub const fn new() -> Self {
        Self {
            load_shedding: LoadSheddingLevel::None,
        }
    }

    /// Sets the current load shedding level.
    #[inline]
    pub fn set_load_shedding(&mut self, level: LoadSheddingLevel) {
        self.load_shedding = level;
    }

    /// Returns the active load shedding level.
    #[inline]
    pub const fn load_shedding(self) -> LoadSheddingLevel {
        self.load_shedding
    }

    /// Evaluates whether an entity in the given tier should be dispatched to observers
    /// during the specified tick, accounting for active load shedding.
    pub fn should_replicate(
        self,
        tier: FrequencyTier,
        tick_index: u64,
        is_state_changed: bool,
    ) -> bool {
        match self.load_shedding {
            LoadSheddingLevel::None => tier.should_dispatch(tick_index, is_state_changed),
            LoadSheddingLevel::Level1 => match tier {
                FrequencyTier::Immediate => tick_index.is_multiple_of(2),
                // Throttled to 1 Hz (once every 20 ticks at 20 Hz)
                FrequencyTier::Mid => tick_index.is_multiple_of(20),
                // Drop distant horizon updates under Level 1 shedding
                FrequencyTier::Horizon => false,
            },
            LoadSheddingLevel::Level2 => match tier {
                // Strictly protect Immediate tier combat responsiveness
                FrequencyTier::Immediate => tick_index.is_multiple_of(2),
                FrequencyTier::Mid => false,
                FrequencyTier::Horizon => false,
            },
        }
    }
}

/// Represents an entity visibility transition event relative to an observer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityEvent {
    /// Entity newly entered the observer's visible Horizon.
    Enter {
        /// Entity identifier.
        entity_id: u32,
        /// Initial assigned frequency tier.
        tier: FrequencyTier,
    },
    /// Entity frequency tier changed due to relative movement.
    TierChange {
        /// Entity identifier.
        entity_id: u32,
        /// Newly assigned frequency tier.
        tier: FrequencyTier,
    },
    /// Entity exited the observer's visible Horizon.
    Exit {
        /// Entity identifier.
        entity_id: u32,
    },
}

/// Pre-allocated interest set tracking visible entities for a single observer.
///
/// Uses double-buffered flat arrays to execute visibility diffs (enters, exits, tier changes)
/// with zero runtime heap allocations.
pub struct ObserverInterestSet {
    max_tracked: usize,
    curr_count: usize,
    prev_count: usize,
    curr_entity_ids: Vec<u32>,
    curr_tiers: Vec<FrequencyTier>,
    prev_entity_ids: Vec<u32>,
    prev_tiers: Vec<FrequencyTier>,
}

impl ObserverInterestSet {
    /// Constructs an observer interest set with pre-allocated capacity.
    pub fn with_capacity(max_tracked: usize) -> Self {
        Self {
            max_tracked,
            curr_count: 0,
            prev_count: 0,
            curr_entity_ids: vec![0; max_tracked],
            curr_tiers: vec![FrequencyTier::Immediate; max_tracked],
            prev_entity_ids: vec![0; max_tracked],
            prev_tiers: vec![FrequencyTier::Immediate; max_tracked],
        }
    }

    /// Returns the number of currently visible entities.
    #[inline]
    pub fn len(&self) -> usize {
        self.curr_count
    }

    /// Returns true if no entities are currently tracked.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.curr_count == 0
    }

    /// Updates tracked visibility from a spatial query result, emitting visibility events
    /// into `event_output` without heap allocation.
    pub fn update_visibility(
        &mut self,
        grid: &SpatialHashGrid,
        observer_pos: Vec3Fix,
        queried_candidates: &[u32],
        event_output: &mut [VisibilityEvent],
    ) -> usize {
        // Swap current and previous buffers
        core::mem::swap(&mut self.curr_entity_ids, &mut self.prev_entity_ids);
        core::mem::swap(&mut self.curr_tiers, &mut self.prev_tiers);
        self.prev_count = self.curr_count;
        self.curr_count = 0;

        let mut event_count = 0;

        // 1. Process candidate entities currently in range
        for &cand_id in queried_candidates {
            if let Some(cand_pos) = grid.get_position(cand_id) {
                let dist_sq = observer_pos.distance_squared(cand_pos);
                if dist_sq <= MAX_AOI_RADIUS_SQ {
                    // Search in previous buffer for hysteresis continuity
                    let prev_idx = self.find_in_prev(cand_id);
                    let new_tier = match prev_idx {
                        Some(idx) => self.prev_tiers[idx].update_with_hysteresis(dist_sq),
                        None => FrequencyTier::classify_initial(dist_sq),
                    };

                    match prev_idx {
                        None => {
                            if event_count < event_output.len() {
                                event_output[event_count] = VisibilityEvent::Enter {
                                    entity_id: cand_id,
                                    tier: new_tier,
                                };
                                event_count += 1;
                            }
                        }
                        Some(idx) => {
                            if self.prev_tiers[idx] != new_tier && event_count < event_output.len()
                            {
                                event_output[event_count] = VisibilityEvent::TierChange {
                                    entity_id: cand_id,
                                    tier: new_tier,
                                };
                                event_count += 1;
                            }
                        }
                    }

                    if self.curr_count < self.max_tracked {
                        self.curr_entity_ids[self.curr_count] = cand_id;
                        self.curr_tiers[self.curr_count] = new_tier;
                        self.curr_count += 1;
                    }
                }
            }
        }

        // 2. Detect exited entities (entities in prev buffer missing from curr buffer)
        for i in 0..self.prev_count {
            let old_id = self.prev_entity_ids[i];
            if !self.contains_in_curr(old_id) && event_count < event_output.len() {
                event_output[event_count] = VisibilityEvent::Exit { entity_id: old_id };
                event_count += 1;
            }
        }

        event_count
    }

    #[inline]
    fn find_in_prev(&self, entity_id: u32) -> Option<usize> {
        (0..self.prev_count).find(|&i| self.prev_entity_ids[i] == entity_id)
    }

    #[inline]
    fn contains_in_curr(&self, entity_id: u32) -> bool {
        (0..self.curr_count).any(|i| self.curr_entity_ids[i] == entity_id)
    }
}
