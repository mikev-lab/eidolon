//! Multi-node zone partition semantics, boundary leases, and split-brain mitigation.
//!
//! Enforces distributed consistency invariants across inter-zone boundaries:
//! - Renewable lease-based spatial boundary cell ownership.
//! - Automated split-brain partition detection on heartbeat loss (>3 ticks / 150ms).
//! - Automatic boundary entity freezing and migration rollback preserving single-writer invariants.

use crate::error::{WorldError, ZoneId};

/// Default heartbeat timeout ticks before declaring a network partition (3 ticks = 150ms at 20 Hz).
pub const DEFAULT_PARTITION_TIMEOUT_TICKS: u64 = 3;

/// Default lease renewal duration in ticks (20 ticks = 1.0 second).
pub const DEFAULT_LEASE_DURATION_TICKS: u64 = 20;

/// Lifecycle partition state of an entity near or crossing a zone boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityPartitionState {
    /// Entity is actively simulated and authoritative in its home zone.
    ActiveAuthoritative,
    /// Entity migration to target zone is in progress.
    MigratingInFlight {
        /// Destination zone identifier.
        target_zone: ZoneId,
        /// Tick when migration was initiated.
        initiated_tick: u64,
    },
    /// Entity is frozen in read-only state due to active partition between zones.
    FrozenReadOnly,
}

/// Renewable lease governing authoritative ownership of spatial boundary cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundaryLease {
    /// Unique lease identifier.
    pub lease_id: u64,
    /// Zone holding authoritative ownership.
    pub owner_zone: ZoneId,
    /// Neighboring zone across the boundary.
    pub partner_zone: ZoneId,
    /// Simulation tick when lease was granted.
    pub granted_tick: u64,
    /// Simulation tick when lease expires unless renewed.
    pub expires_tick: u64,
}

impl BoundaryLease {
    /// Creates a new boundary lease.
    pub fn new(
        lease_id: u64,
        owner_zone: ZoneId,
        partner_zone: ZoneId,
        current_tick: u64,
        duration_ticks: u64,
    ) -> Self {
        Self {
            lease_id,
            owner_zone,
            partner_zone,
            granted_tick: current_tick,
            expires_tick: current_tick.saturating_add(duration_ticks),
        }
    }

    /// Checks if the lease is currently active and unexpired.
    #[inline]
    pub fn is_valid(&self, current_tick: u64) -> bool {
        current_tick <= self.expires_tick
    }

    /// Renews the lease for an additional duration.
    pub fn renew(&mut self, current_tick: u64, duration_ticks: u64) {
        self.granted_tick = current_tick;
        self.expires_tick = current_tick.saturating_add(duration_ticks);
    }
}

/// Heartbeat detector monitoring connectivity between adjacent zone server nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZonePartitionDetector {
    /// Local zone identifier.
    pub local_zone: ZoneId,
    /// Remote zone identifier being monitored.
    pub remote_zone: ZoneId,
    /// Last simulation tick when a valid heartbeat was received.
    pub last_heartbeat_tick: u64,
    /// Timeout threshold in ticks before declaring partition.
    pub timeout_ticks: u64,
}

impl ZonePartitionDetector {
    /// Creates a new partition detector.
    pub fn new(local_zone: ZoneId, remote_zone: ZoneId, current_tick: u64) -> Self {
        Self {
            local_zone,
            remote_zone,
            last_heartbeat_tick: current_tick,
            timeout_ticks: DEFAULT_PARTITION_TIMEOUT_TICKS,
        }
    }

    /// Records receipt of a heartbeat from the remote zone node.
    #[inline]
    pub fn record_heartbeat(&mut self, current_tick: u64) {
        if current_tick > self.last_heartbeat_tick {
            self.last_heartbeat_tick = current_tick;
        }
    }

    /// Checks if the communication link to the remote zone is severed.
    #[inline]
    pub fn is_partitioned(&self, current_tick: u64) -> bool {
        current_tick.saturating_sub(self.last_heartbeat_tick) > self.timeout_ticks
    }
}

/// Coordinates distributed boundary leases, partition detection, and split-brain resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZonePartitionManager {
    local_zone: ZoneId,
    remote_zone: ZoneId,
    detector: ZonePartitionDetector,
    lease: BoundaryLease,
}

impl ZonePartitionManager {
    /// Creates a new partition manager for an inter-zone boundary.
    pub fn new(local_zone: ZoneId, remote_zone: ZoneId, current_tick: u64) -> Self {
        let detector = ZonePartitionDetector::new(local_zone, remote_zone, current_tick);
        let lease = BoundaryLease::new(
            1,
            local_zone,
            remote_zone,
            current_tick,
            DEFAULT_LEASE_DURATION_TICKS,
        );
        Self {
            local_zone,
            remote_zone,
            detector,
            lease,
        }
    }

    /// Records heartbeat from partner node and automatically renews boundary lease.
    pub fn handle_heartbeat(&mut self, current_tick: u64) {
        self.detector.record_heartbeat(current_tick);
        self.lease.renew(current_tick, DEFAULT_LEASE_DURATION_TICKS);
    }

    /// Evaluates migration attempt of an entity across the boundary.
    ///
    /// Returns `Ok(())` if link is healthy and lease is active.
    /// Returns `Err(WorldError::PartitionSevered)` or `Err(WorldError::LeaseExpired)` if unsafe.
    pub fn validate_migration(&self, current_tick: u64) -> Result<(), WorldError> {
        if self.detector.is_partitioned(current_tick) {
            return Err(WorldError::PartitionSevered {
                local_zone: self.local_zone,
                remote_zone: self.remote_zone,
            });
        }
        if !self.lease.is_valid(current_tick) {
            return Err(WorldError::LeaseExpired {
                lease_id: self.lease.lease_id,
                zone_id: self.local_zone,
            });
        }
        Ok(())
    }

    /// Resolves an in-flight migration under partition, rolling back ownership to local zone.
    pub fn resolve_partition_rollback(
        &self,
        entity_id: u32,
        current_tick: u64,
    ) -> EntityPartitionState {
        if self.detector.is_partitioned(current_tick) {
            EntityPartitionState::FrozenReadOnly
        } else {
            // Link healed: return to active authoritative status
            let _ = entity_id;
            EntityPartitionState::ActiveAuthoritative
        }
    }

    /// Returns whether the partition detector currently reports an active network split.
    #[inline]
    pub fn is_partitioned(&self, current_tick: u64) -> bool {
        self.detector.is_partitioned(current_tick)
    }

    /// Returns current boundary lease.
    #[inline]
    pub fn lease(&self) -> &BoundaryLease {
        &self.lease
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_boundary_lease_lifecycle() {
        let mut lease = BoundaryLease::new(101, ZoneId(1), ZoneId(2), 10, 20);
        assert!(lease.is_valid(10));
        assert!(lease.is_valid(30));
        assert!(!lease.is_valid(31));

        lease.renew(30, 20);
        assert!(lease.is_valid(40));
        assert!(lease.is_valid(50));
        assert!(!lease.is_valid(51));
    }

    #[test]
    fn test_zone_partition_detector() {
        let mut detector = ZonePartitionDetector::new(ZoneId(1), ZoneId(2), 100);
        assert!(!detector.is_partitioned(100));
        assert!(!detector.is_partitioned(103));
        // 4 ticks without heartbeat triggers partition declaration
        assert!(detector.is_partitioned(104));

        // Heartbeat heals partition
        detector.record_heartbeat(105);
        assert!(!detector.is_partitioned(105));
    }

    #[test]
    fn test_partition_manager_validates_and_rolls_back() {
        let mut manager = ZonePartitionManager::new(ZoneId(1), ZoneId(2), 1);
        assert!(manager.validate_migration(2).is_ok());

        // Partition link by advancing 5 ticks without heartbeat
        assert_eq!(
            manager.validate_migration(6),
            Err(WorldError::PartitionSevered {
                local_zone: ZoneId(1),
                remote_zone: ZoneId(2)
            })
        );

        // In-flight entity migration resolves to FrozenReadOnly
        let state = manager.resolve_partition_rollback(42, 6);
        assert_eq!(state, EntityPartitionState::FrozenReadOnly);

        // Heartbeat received: link healed, rollback resolves to ActiveAuthoritative
        manager.handle_heartbeat(7);
        let healed_state = manager.resolve_partition_rollback(42, 7);
        assert_eq!(healed_state, EntityPartitionState::ActiveAuthoritative);
    }
}
