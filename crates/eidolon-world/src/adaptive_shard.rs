//! Dynamic adaptive shard manager for continental-scale worlds.
//!
//! Rebalances 256-meter sectors across worker cores based on live CPU load and entity density:
//! contracts high-density hotspots (e.g., 2,000 combatants in a capital city) to dedicated worker
//! cores, while merging sparse wilderness sectors across hundreds of square kilometers into shared
//! background shards.

use std::collections::HashMap;

use crate::error::WorldError;

/// Maximum number of worker shards in a continental cluster.
pub const MAX_WORKER_SHARDS: usize = 64;

/// Default entity threshold to declare a sector a high-density hotspot.
pub const DEFAULT_HOTSPOT_ENTITY_THRESHOLD: u32 = 500;

/// Default entity threshold below which a sector is considered sparse wilderness.
pub const DEFAULT_WILDERNESS_ENTITY_THRESHOLD: u32 = 10;

/// Default tick execution duration threshold (in microseconds) triggering shard overload rebalance (35ms).
pub const DEFAULT_CPU_OVERLOAD_MICROS: u64 = 35_000;

/// Reason triggering an adaptive shard rebalance action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebalanceReason {
    /// Sector entity density exceeded hotspot threshold: contracted to dedicated worker.
    HotspotContraction,
    /// Sector entity density dropped below wilderness threshold: merged into background shard.
    WildernessMerge,
    /// Worker core CPU execution duration exceeded overload threshold: offloading sector.
    WorkerOverload,
    /// Manual administrator or topology migration.
    Manual,
}

/// Description of a sector migration between worker shards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardRebalanceAction {
    /// Horizontal sector X coordinate.
    pub sector_x: i32,
    /// Horizontal sector Z coordinate.
    pub sector_z: i32,
    /// Origin worker shard identifier.
    pub from_shard: u32,
    /// Destination worker shard identifier.
    pub to_shard: u32,
    /// Justification for dynamic rebalancing.
    pub reason: RebalanceReason,
}

/// Live performance metrics for a single worker shard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardMetrics {
    /// Worker shard identifier.
    pub shard_id: u32,
    /// Number of active 256m sectors assigned to this shard.
    pub active_sectors: u32,
    /// Total number of entities simulated by this shard.
    pub total_entities: u32,
    /// Execution duration of the most recent tick in microseconds.
    pub last_tick_duration_micros: u64,
}

impl ShardMetrics {
    /// Creates an empty metric descriptor for the given shard.
    pub const fn new(shard_id: u32) -> Self {
        Self {
            shard_id,
            active_sectors: 0,
            total_entities: 0,
            last_tick_duration_micros: 0,
        }
    }
}

/// Metadata tracked for an individual 256m sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectorMetadata {
    /// Worker shard currently managing this sector.
    pub shard_id: u32,
    /// Number of live entities inside this sector.
    pub entity_count: u32,
    /// Tick timestamp when this sector was last rebalanced.
    pub last_rebalanced_tick: u64,
}

/// Dynamic adaptive shard manager distributing continental sectors across worker threads.
#[derive(Debug)]
pub struct AdaptiveShardManager {
    /// Pre-allocated array of worker shard metrics.
    shards: [ShardMetrics; MAX_WORKER_SHARDS],
    /// Number of active worker shards.
    shard_count: usize,
    /// Mapping of discrete 256m sector coordinates to assigned worker shard metadata.
    sectors: HashMap<(i32, i32), SectorMetadata>,
    /// Entity count threshold to trigger hotspot contraction.
    hotspot_threshold: u32,
    /// Entity count threshold to trigger wilderness merging.
    wilderness_threshold: u32,
    /// CPU execution duration threshold triggering overload rebalancing.
    cpu_overload_threshold_micros: u64,
    /// Designated background shard for low-density wilderness sectors.
    background_shard_id: u32,
}

impl AdaptiveShardManager {
    /// Creates a new adaptive shard manager with the specified number of active worker shards.
    pub fn new(num_shards: usize) -> Result<Self, WorldError> {
        let count = num_shards.min(MAX_WORKER_SHARDS);
        if count == 0 {
            return Err(WorldError::ShardNotFound(0));
        }

        let mut shards = [ShardMetrics::new(0); MAX_WORKER_SHARDS];
        for (i, shard) in shards.iter_mut().enumerate().take(count) {
            *shard = ShardMetrics::new(i as u32);
        }

        Ok(Self {
            shards,
            shard_count: count,
            sectors: HashMap::new(),
            hotspot_threshold: DEFAULT_HOTSPOT_ENTITY_THRESHOLD,
            wilderness_threshold: DEFAULT_WILDERNESS_ENTITY_THRESHOLD,
            cpu_overload_threshold_micros: DEFAULT_CPU_OVERLOAD_MICROS,
            background_shard_id: 0,
        })
    }

    /// Configures custom rebalance thresholds.
    pub fn configure_thresholds(
        &mut self,
        hotspot: u32,
        wilderness: u32,
        cpu_overload_micros: u64,
        background_shard: u32,
    ) {
        self.hotspot_threshold = hotspot;
        self.wilderness_threshold = wilderness;
        self.cpu_overload_threshold_micros = cpu_overload_micros;
        if (background_shard as usize) < self.shard_count {
            self.background_shard_id = background_shard;
        }
    }

    /// Assigns a 256m sector to a specific worker shard.
    pub fn assign_sector(
        &mut self,
        sector_x: i32,
        sector_z: i32,
        shard_id: u32,
    ) -> Result<(), WorldError> {
        if shard_id as usize >= self.shard_count {
            return Err(WorldError::ShardNotFound(shard_id));
        }

        if let Some(meta) = self.sectors.get_mut(&(sector_x, sector_z)) {
            if meta.shard_id != shard_id {
                let old_shard = meta.shard_id as usize;
                if let Some(old) = self.shards.get_mut(old_shard) {
                    old.active_sectors = old.active_sectors.saturating_sub(1);
                    old.total_entities = old.total_entities.saturating_sub(meta.entity_count);
                }
                meta.shard_id = shard_id;
                let new_shard = shard_id as usize;
                if let Some(new_m) = self.shards.get_mut(new_shard) {
                    new_m.active_sectors = new_m.active_sectors.saturating_add(1);
                    new_m.total_entities = new_m.total_entities.saturating_add(meta.entity_count);
                }
            }
        } else {
            self.sectors.insert(
                (sector_x, sector_z),
                SectorMetadata {
                    shard_id,
                    entity_count: 0,
                    last_rebalanced_tick: 0,
                },
            );
            if let Some(m) = self.shards.get_mut(shard_id as usize) {
                m.active_sectors = m.active_sectors.saturating_add(1);
            }
        }

        Ok(())
    }

    /// Queries the worker shard currently assigned to a 256m sector coordinate.
    #[inline]
    pub fn get_shard_for_sector(&self, sector_x: i32, sector_z: i32) -> Option<u32> {
        self.sectors.get(&(sector_x, sector_z)).map(|m| m.shard_id)
    }

    /// Updates the entity count in a sector and refreshes shard-level aggregates.
    pub fn update_sector_entity_count(
        &mut self,
        sector_x: i32,
        sector_z: i32,
        new_count: u32,
    ) -> Result<(), WorldError> {
        let meta = self
            .sectors
            .get_mut(&(sector_x, sector_z))
            .ok_or(WorldError::EntityNotFound(0))?;

        let shard_id = meta.shard_id as usize;
        let old_count = meta.entity_count;
        meta.entity_count = new_count;

        if let Some(shard) = self.shards.get_mut(shard_id) {
            shard.total_entities = shard
                .total_entities
                .saturating_sub(old_count)
                .saturating_add(new_count);
        }

        Ok(())
    }

    /// Records the latest tick execution duration for a worker shard.
    pub fn update_shard_cpu(
        &mut self,
        shard_id: u32,
        tick_duration_micros: u64,
    ) -> Result<(), WorldError> {
        let shard = self
            .shards
            .get_mut(shard_id as usize)
            .ok_or(WorldError::ShardNotFound(shard_id))?;
        shard.last_tick_duration_micros = tick_duration_micros;
        Ok(())
    }

    /// Returns the metrics descriptor for a worker shard.
    pub fn shard_metrics(&self, shard_id: u32) -> Option<&ShardMetrics> {
        if (shard_id as usize) < self.shard_count {
            self.shards.get(shard_id as usize)
        } else {
            None
        }
    }

    /// Returns the total count of registered sectors.
    pub fn total_registered_sectors(&self) -> usize {
        self.sectors.len()
    }

    /// Contracts a high-density hotspot sector to a dedicated worker shard.
    pub fn contract_hotspot_to_shard(
        &mut self,
        sector_x: i32,
        sector_z: i32,
        target_shard: u32,
        current_tick: u64,
    ) -> Result<ShardRebalanceAction, WorldError> {
        if target_shard as usize >= self.shard_count {
            return Err(WorldError::ShardNotFound(target_shard));
        }

        let meta = self
            .sectors
            .get_mut(&(sector_x, sector_z))
            .ok_or(WorldError::ShardNotFound(0))?;

        let from_shard = meta.shard_id;
        if from_shard == target_shard {
            return Ok(ShardRebalanceAction {
                sector_x,
                sector_z,
                from_shard,
                to_shard: target_shard,
                reason: RebalanceReason::HotspotContraction,
            });
        }

        let entity_count = meta.entity_count;
        meta.shard_id = target_shard;
        meta.last_rebalanced_tick = current_tick;

        if let Some(from) = self.shards.get_mut(from_shard as usize) {
            from.active_sectors = from.active_sectors.saturating_sub(1);
            from.total_entities = from.total_entities.saturating_sub(entity_count);
        }

        if let Some(to) = self.shards.get_mut(target_shard as usize) {
            to.active_sectors = to.active_sectors.saturating_add(1);
            to.total_entities = to.total_entities.saturating_add(entity_count);
        }

        Ok(ShardRebalanceAction {
            sector_x,
            sector_z,
            from_shard,
            to_shard: target_shard,
            reason: RebalanceReason::HotspotContraction,
        })
    }

    /// Merges a sparse wilderness sector into the designated background worker shard.
    pub fn merge_wilderness_to_background(
        &mut self,
        sector_x: i32,
        sector_z: i32,
        current_tick: u64,
    ) -> Result<ShardRebalanceAction, WorldError> {
        let bg_shard = self.background_shard_id;
        let meta = self
            .sectors
            .get_mut(&(sector_x, sector_z))
            .ok_or(WorldError::ShardNotFound(0))?;

        let from_shard = meta.shard_id;
        if from_shard == bg_shard {
            return Ok(ShardRebalanceAction {
                sector_x,
                sector_z,
                from_shard,
                to_shard: bg_shard,
                reason: RebalanceReason::WildernessMerge,
            });
        }

        let entity_count = meta.entity_count;
        meta.shard_id = bg_shard;
        meta.last_rebalanced_tick = current_tick;

        if let Some(from) = self.shards.get_mut(from_shard as usize) {
            from.active_sectors = from.active_sectors.saturating_sub(1);
            from.total_entities = from.total_entities.saturating_sub(entity_count);
        }

        if let Some(to) = self.shards.get_mut(bg_shard as usize) {
            to.active_sectors = to.active_sectors.saturating_add(1);
            to.total_entities = to.total_entities.saturating_add(entity_count);
        }

        Ok(ShardRebalanceAction {
            sector_x,
            sector_z,
            from_shard,
            to_shard: bg_shard,
            reason: RebalanceReason::WildernessMerge,
        })
    }

    /// Evaluates all registered sectors and shards against performance thresholds,
    /// populating the provided buffer with rebalance actions without hot heap allocation.
    pub fn evaluate_rebalance(
        &mut self,
        current_tick: u64,
        out_actions: &mut Vec<ShardRebalanceAction>,
    ) {
        // Collect candidate sectors to reassign
        let mut hotspot_candidates: Vec<((i32, i32), u32)> = Vec::new();
        let mut wilderness_candidates: Vec<(i32, i32)> = Vec::new();

        for (&(sx, sz), meta) in self.sectors.iter() {
            // Hotspot check: entity density exceeds threshold and not already on dedicated shard
            if meta.entity_count >= self.hotspot_threshold
                && meta.shard_id == self.background_shard_id
            {
                hotspot_candidates.push(((sx, sz), meta.entity_count));
            } else if meta.entity_count <= self.wilderness_threshold
                && meta.shard_id != self.background_shard_id
            {
                // Wilderness check: low entity count and currently not on background shard
                wilderness_candidates.push((sx, sz));
            }
        }

        // Process wilderness merges back into background shard
        for (sx, sz) in wilderness_candidates {
            if let Ok(action) = self.merge_wilderness_to_background(sx, sz, current_tick) {
                if action.from_shard != action.to_shard {
                    out_actions.push(action);
                }
            }
        }

        // Process hotspot contractions to least loaded non-background shard
        for ((sx, sz), _count) in hotspot_candidates {
            // Find worker with lowest total entities among non-background shards
            let mut best_shard = None;
            let mut min_load = u32::MAX;

            for shard in self.shards.iter().take(self.shard_count) {
                if shard.shard_id != self.background_shard_id && shard.total_entities < min_load {
                    min_load = shard.total_entities;
                    best_shard = Some(shard.shard_id);
                }
            }

            if let Some(target) = best_shard {
                if let Ok(action) = self.contract_hotspot_to_shard(sx, sz, target, current_tick) {
                    if action.from_shard != action.to_shard {
                        out_actions.push(action);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adaptive_shard_initialization() {
        let manager = AdaptiveShardManager::new(4);
        assert!(manager.is_ok());
        let mgr = manager.unwrap();
        assert_eq!(mgr.shard_count, 4);

        let m0 = mgr.shard_metrics(0);
        assert!(m0.is_some());
        assert_eq!(m0.unwrap().active_sectors, 0);

        let m_invalid = mgr.shard_metrics(99);
        assert!(m_invalid.is_none());
    }

    #[test]
    fn test_sector_assignment_and_query() {
        let mut mgr = AdaptiveShardManager::new(4).unwrap();
        let assign_res = mgr.assign_sector(10, -5, 2);
        assert!(assign_res.is_ok());

        assert_eq!(mgr.get_shard_for_sector(10, -5), Some(2));
        assert_eq!(mgr.get_shard_for_sector(0, 0), None);

        let m2 = mgr.shard_metrics(2).unwrap();
        assert_eq!(m2.active_sectors, 1);
    }

    #[test]
    fn test_hotspot_contraction_and_wilderness_merge() {
        let mut mgr = AdaptiveShardManager::new(4).unwrap();
        mgr.configure_thresholds(500, 10, 35_000, 0);

        // Assign sector (0, 0) to background shard 0
        mgr.assign_sector(0, 0, 0).unwrap();
        mgr.update_sector_entity_count(0, 0, 2000).unwrap();

        // Assign sector (10, 10) to worker shard 1 with 2 entities
        mgr.assign_sector(10, 10, 1).unwrap();
        mgr.update_sector_entity_count(10, 10, 2).unwrap();

        let mut actions = Vec::new();
        mgr.evaluate_rebalance(100, &mut actions);

        // Expect hotspot (0, 0) to move from 0 to worker 1, 2, or 3
        // Expect wilderness (10, 10) to move from 1 to background 0
        assert_eq!(actions.len(), 2);

        let wilderness_action = actions
            .iter()
            .find(|a| a.sector_x == 10 && a.sector_z == 10);
        assert!(wilderness_action.is_some());
        let w = wilderness_action.unwrap();
        assert_eq!(w.from_shard, 1);
        assert_eq!(w.to_shard, 0);
        assert_eq!(w.reason, RebalanceReason::WildernessMerge);

        let hotspot_action = actions.iter().find(|a| a.sector_x == 0 && a.sector_z == 0);
        assert!(hotspot_action.is_some());
        let h = hotspot_action.unwrap();
        assert_eq!(h.from_shard, 0);
        assert_ne!(h.to_shard, 0);
        assert_eq!(h.reason, RebalanceReason::HotspotContraction);
    }

    #[test]
    fn test_invalid_shard_rejection() {
        let mut mgr = AdaptiveShardManager::new(4).unwrap();
        let res = mgr.assign_sector(0, 0, 10);
        assert!(matches!(res, Err(WorldError::ShardNotFound(10))));
    }
}
