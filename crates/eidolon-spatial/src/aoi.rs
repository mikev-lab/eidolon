//! Visibility queries, observer interest management, and adaptive load shedding.
//!
//! Orchestrates dynamic multi-tier entity replication across simulation ticks,
//! shedding non-essential updates under CPU overruns to preserve simulation stability.

use crate::grid::SpatialHashGrid;
use crate::tier::{FrequencyTier, TierModel};
use core::fmt;
use eidolon_core::fixed::{Fixed64, Vec3Fix};

/// Maximum horizontal Area of Interest radius in meters (50 meters).
pub const MAX_AOI_RADIUS_METERS: f64 = 50.0;

/// Maximum observable radius squared for Area of Interest in standard 3-tier mode (50m squared = 2500 m^2).
pub const MAX_AOI_RADIUS_SQ: Fixed64 = Fixed64::from_i32(2500);

/// Extended maximum observable radius squared for extreme density 5-tier mode (300m squared = 90,000 m^2).
pub const MAX_AOI_5TIER_RADIUS_SQ: Fixed64 = Fixed64::from_i32(90000);

/// Configurable weights for continuous priority relevance scoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelevanceWeights {
    /// Bonus points for being in the same party.
    pub party_bonus: u32,
    /// Bonus points for being the observer's active combat target.
    pub target_bonus: u32,
    /// Bonus points for being an aggressive or attacking threat.
    pub threat_bonus: u32,
    /// Bonus points for sharing guild or faction affiliation.
    pub guild_bonus: u32,
}

impl Default for RelevanceWeights {
    fn default() -> Self {
        Self {
            party_bonus: 1_000_000,
            target_bonus: 2_000_000,
            threat_bonus: 500_000,
            guild_bonus: 100_000,
        }
    }
}

/// Context relation flags for an entity relative to an observer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EntityRelation {
    /// True if the entity is in the observer's party.
    pub is_party: bool,
    /// True if the entity is the observer's targeted focus.
    pub is_target: bool,
    /// True if the entity is attacking or hostile to the observer.
    pub is_threat: bool,
    /// True if the entity belongs to the observer's guild or faction.
    pub is_guild: bool,
}

/// Calculates continuous priority relevance score for an entity relative to an observer.
/// Combines inverse square distance with relationship bonuses. Higher scores indicate higher priority.
#[inline]
pub fn calculate_relevance_score(
    dist_sq: Fixed64,
    relation: EntityRelation,
    weights: &RelevanceWeights,
) -> u64 {
    let d_sq = dist_sq.to_i32().max(0) as u64;
    // Base distance falloff: inverse curve scaled up
    let dist_score = 100_000_000 / (1 + d_sq);

    let mut score = dist_score;
    if relation.is_target {
        score += weights.target_bonus as u64;
    }
    if relation.is_party {
        score += weights.party_bonus as u64;
    }
    if relation.is_threat {
        score += weights.threat_bonus as u64;
    }
    if relation.is_guild {
        score += weights.guild_bonus as u64;
    }
    score
}

/// Dynamic degradation levels triggered during CPU tick overrun shedding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum LoadSheddingLevel {
    /// Normal operation: full replication rates across all tiers.
    #[default]
    None,
    /// Level 1 degradation: throttles mid/tactical tiers and suppresses horizon updates.
    Level1,
    /// Level 2 degradation: strictly protects Immediate combat tier, suppressing all distant tiers.
    Level2,
}

impl fmt::Display for LoadSheddingLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::Level1 => write!(f, "Level 1 (Mid Throttled)"),
            Self::Level2 => write!(f, "Level 2 (Immediate Only)"),
        }
    }
}

/// Dynamic Area of Interest update scheduler and load-shedding regulator.
#[derive(Debug, Clone, Copy, Default)]
pub struct AoIScheduler {
    load_shedding: LoadSheddingLevel,
}

impl AoIScheduler {
    /// Constructs a scheduler operating under normal load.
    pub const fn new() -> Self {
        Self {
            load_shedding: LoadSheddingLevel::None,
        }
    }

    /// Sets the active load shedding degradation level.
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
                FrequencyTier::Tactical => tick_index.is_multiple_of(4),
                // Throttled to 1 Hz (once every 20 ticks at 20 Hz)
                FrequencyTier::Mid => tick_index.is_multiple_of(20),
                // Drop distant horizon updates under Level 1 shedding
                FrequencyTier::Horizon | FrequencyTier::Macro => false,
            },
            LoadSheddingLevel::Level2 => match tier {
                // Strictly protect Immediate tier combat responsiveness
                FrequencyTier::Immediate => tick_index.is_multiple_of(2),
                FrequencyTier::Tactical
                | FrequencyTier::Mid
                | FrequencyTier::Horizon
                | FrequencyTier::Macro => false,
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
    tier_model: TierModel,
    max_radius_sq: Fixed64,
    relevance_weights: RelevanceWeights,
    scratch_ranked: Vec<(u64, u32)>,
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
            tier_model: TierModel::Standard3Tier,
            max_radius_sq: MAX_AOI_RADIUS_SQ,
            relevance_weights: RelevanceWeights::default(),
            scratch_ranked: vec![(0, 0); max_tracked.max(64) * 4],
        }
    }

    /// Constructs an observer interest set with explicit tier model and maximum radius.
    pub fn with_capacity_and_model(
        max_tracked: usize,
        tier_model: TierModel,
        max_radius_sq: Fixed64,
    ) -> Self {
        Self {
            max_tracked,
            curr_count: 0,
            prev_count: 0,
            curr_entity_ids: vec![0; max_tracked],
            curr_tiers: vec![FrequencyTier::Immediate; max_tracked],
            prev_entity_ids: vec![0; max_tracked],
            prev_tiers: vec![FrequencyTier::Immediate; max_tracked],
            tier_model,
            max_radius_sq,
            relevance_weights: RelevanceWeights::default(),
            scratch_ranked: vec![(0, 0); max_tracked.max(64) * 4],
        }
    }

    /// Configures the active tier model (Standard3Tier or Continuous5Tier).
    pub fn set_tier_model(&mut self, tier_model: TierModel) {
        self.tier_model = tier_model;
    }

    /// Returns the active tier model.
    #[inline]
    pub const fn tier_model(&self) -> TierModel {
        self.tier_model
    }

    /// Sets the maximum Area of Interest squared radius.
    pub fn set_max_radius_sq(&mut self, max_radius_sq: Fixed64) {
        self.max_radius_sq = max_radius_sq;
    }

    /// Returns the maximum Area of Interest squared radius.
    #[inline]
    pub const fn max_radius_sq(&self) -> Fixed64 {
        self.max_radius_sq
    }

    /// Sets the relevance scoring weights.
    pub fn set_relevance_weights(&mut self, weights: RelevanceWeights) {
        self.relevance_weights = weights;
    }

    /// Returns the relevance scoring weights.
    #[inline]
    pub const fn relevance_weights(&self) -> &RelevanceWeights {
        &self.relevance_weights
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

    /// Returns the slice of currently tracked entity IDs in sorted order.
    #[inline]
    pub fn visible_entities(&self) -> &[u32] {
        &self.curr_entity_ids[..self.curr_count]
    }

    /// Returns the slice of frequency tiers corresponding to currently tracked entity IDs.
    #[inline]
    pub fn visible_tiers(&self) -> &[FrequencyTier] {
        &self.curr_tiers[..self.curr_count]
    }

    /// Updates tracked visibility from a spatial query result, emitting visibility events
    /// into `event_output` without heap allocation.
    ///
    /// Executes in O(N + M) time using sorted double-buffered arrays, binary search hysteresis lookups,
    /// and two-pointer merge sweeps.
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
        let is_5tier = self.tier_model == TierModel::Continuous5Tier;
        let max_r_sq = self.max_radius_sq;

        // 1. Process candidate entities currently in range
        for &cand_id in queried_candidates {
            if let Some(cand_pos) = grid.get_position(cand_id) {
                let dist_sq = observer_pos.distance_squared(cand_pos);
                if dist_sq <= max_r_sq {
                    // Search in sorted previous buffer for hysteresis continuity in O(log M)
                    let prev_idx = self.find_in_prev(cand_id);
                    let new_tier = match prev_idx {
                        Some(idx) => {
                            if is_5tier {
                                self.prev_tiers[idx].update_5tier_with_hysteresis(dist_sq)
                            } else {
                                self.prev_tiers[idx].update_with_hysteresis(dist_sq)
                            }
                        }
                        None => {
                            if is_5tier {
                                FrequencyTier::classify_5tier(dist_sq)
                            } else {
                                FrequencyTier::classify_initial(dist_sq)
                            }
                        }
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

                    // Maintain sorted order in curr buffers for O(N + M) diffing and O(log N) lookup
                    match self.curr_entity_ids[..self.curr_count].binary_search(&cand_id) {
                        Ok(existing_idx) => {
                            self.curr_tiers[existing_idx] = new_tier;
                        }
                        Err(insert_idx) => {
                            if self.curr_count < self.max_tracked {
                                self.curr_entity_ids
                                    .copy_within(insert_idx..self.curr_count, insert_idx + 1);
                                self.curr_tiers
                                    .copy_within(insert_idx..self.curr_count, insert_idx + 1);
                                self.curr_entity_ids[insert_idx] = cand_id;
                                self.curr_tiers[insert_idx] = new_tier;
                                self.curr_count += 1;
                            }
                        }
                    }
                }
            }
        }

        // 2. Detect exited entities using an O(N + M) two-pointer merge sweep
        let mut prev_idx = 0;
        let mut curr_idx = 0;
        while prev_idx < self.prev_count && curr_idx < self.curr_count {
            let prev_id = self.prev_entity_ids[prev_idx];
            let curr_id = self.curr_entity_ids[curr_idx];
            if prev_id < curr_id {
                // prev_id is not in curr: it has exited
                if event_count < event_output.len() {
                    event_output[event_count] = VisibilityEvent::Exit { entity_id: prev_id };
                    event_count += 1;
                }
                prev_idx += 1;
            } else if prev_id > curr_id {
                curr_idx += 1;
            } else {
                // Entity is present in both sets
                prev_idx += 1;
                curr_idx += 1;
            }
        }

        while prev_idx < self.prev_count {
            let prev_id = self.prev_entity_ids[prev_idx];
            if event_count < event_output.len() {
                event_output[event_count] = VisibilityEvent::Exit { entity_id: prev_id };
                event_count += 1;
            }
            prev_idx += 1;
        }

        event_count
    }

    /// Updates tracked visibility using priority relevance ranking.
    ///
    /// When candidates exceed `max_tracked`, evaluates `calculate_relevance_score` using
    /// the caller-provided relationship lookup, retaining only the highest-scoring entities.
    pub fn update_visibility_ranked<F>(
        &mut self,
        grid: &SpatialHashGrid,
        observer_pos: Vec3Fix,
        queried_candidates: &[u32],
        relation_lookup: F,
        event_output: &mut [VisibilityEvent],
    ) -> usize
    where
        F: Fn(u32) -> EntityRelation,
    {
        let max_r_sq = self.max_radius_sq;
        let mut valid_count = 0;
        let scratch_cap = self.scratch_ranked.len();

        for &cand_id in queried_candidates {
            if let Some(cand_pos) = grid.get_position(cand_id) {
                let dist_sq = observer_pos.distance_squared(cand_pos);
                if dist_sq <= max_r_sq && valid_count < scratch_cap {
                    let rel = relation_lookup(cand_id);
                    let score = calculate_relevance_score(dist_sq, rel, &self.relevance_weights);
                    self.scratch_ranked[valid_count] = (score, cand_id);
                    valid_count += 1;
                }
            }
        }

        // If candidate count exceeds max_tracked, sort by score descending
        if valid_count > self.max_tracked {
            self.scratch_ranked[..valid_count]
                .sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            valid_count = self.max_tracked;
        }

        // Sort retained candidates by entity ID for sorted double-buffer insertion
        self.scratch_ranked[..valid_count].sort_unstable_by_key(|entry| entry.1);

        // Swap buffers
        core::mem::swap(&mut self.curr_entity_ids, &mut self.prev_entity_ids);
        core::mem::swap(&mut self.curr_tiers, &mut self.prev_tiers);
        self.prev_count = self.curr_count;
        self.curr_count = 0;

        let mut event_count = 0;
        let is_5tier = self.tier_model == TierModel::Continuous5Tier;

        for i in 0..valid_count {
            let cand_id = self.scratch_ranked[i].1;
            if let Some(cand_pos) = grid.get_position(cand_id) {
                let dist_sq = observer_pos.distance_squared(cand_pos);
                let prev_idx = self.find_in_prev(cand_id);
                let new_tier = match prev_idx {
                    Some(idx) => {
                        if is_5tier {
                            self.prev_tiers[idx].update_5tier_with_hysteresis(dist_sq)
                        } else {
                            self.prev_tiers[idx].update_with_hysteresis(dist_sq)
                        }
                    }
                    None => {
                        if is_5tier {
                            FrequencyTier::classify_5tier(dist_sq)
                        } else {
                            FrequencyTier::classify_initial(dist_sq)
                        }
                    }
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
                        if self.prev_tiers[idx] != new_tier && event_count < event_output.len() {
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

        // Two-pointer exit sweep
        let mut prev_idx = 0;
        let mut curr_idx = 0;
        while prev_idx < self.prev_count && curr_idx < self.curr_count {
            let prev_id = self.prev_entity_ids[prev_idx];
            let curr_id = self.curr_entity_ids[curr_idx];
            if prev_id < curr_id {
                if event_count < event_output.len() {
                    event_output[event_count] = VisibilityEvent::Exit { entity_id: prev_id };
                    event_count += 1;
                }
                prev_idx += 1;
            } else if prev_id > curr_id {
                curr_idx += 1;
            } else {
                prev_idx += 1;
                curr_idx += 1;
            }
        }

        while prev_idx < self.prev_count {
            let prev_id = self.prev_entity_ids[prev_idx];
            if event_count < event_output.len() {
                event_output[event_count] = VisibilityEvent::Exit { entity_id: prev_id };
                event_count += 1;
            }
            prev_idx += 1;
        }

        event_count
    }

    /// Returns true if the specified entity ID is currently visible to the observer.
    #[inline]
    pub fn contains(&self, entity_id: u32) -> bool {
        self.curr_entity_ids[..self.curr_count]
            .binary_search(&entity_id)
            .is_ok()
    }

    #[inline]
    fn find_in_prev(&self, entity_id: u32) -> Option<usize> {
        self.prev_entity_ids[..self.prev_count]
            .binary_search(&entity_id)
            .ok()
    }
}
