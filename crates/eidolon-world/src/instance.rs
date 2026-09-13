//! Ephemeral dungeon instances, gacha co-op rooms, and scale-to-zero compute pools.
//!
//! Provides deterministic 4-stage instance lifecycles with sub-50ms allocation
//! from pre-allocated memory pools and zero idle compute costs.

use crate::error::{InstanceId, WorldError};

/// Maximum number of players allowed in a single ephemeral co-op raid instance.
pub const MAX_PARTY_MEMBERS: usize = 4;

/// Timeout in ticks (60 seconds at 20 Hz = 1200 ticks) before an empty or cleared room is purged.
pub const ROOM_CLEANUP_TIMEOUT_TICKS: u32 = 1200;

/// Four-stage lifecycle state machine for an ephemeral dungeon room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceLifecycle {
    /// Room slot claimed from memory pool and awaiting party connections.
    Allocated,
    /// Active combat simulation with connected players and spawned entities.
    ActiveCombat,
    /// Objective completed or boss defeated; grace period for reward distribution.
    VictoryReward,
    /// Room abandoned or expired; scheduled for immediate slot reclamation.
    PendingCleanup,
}

/// Ephemeral dungeon instance simulating an isolated co-op battle.
#[derive(Debug)]
pub struct DungeonInstance {
    /// Unique instance identifier.
    pub id: InstanceId,
    /// Current lifecycle state.
    pub state: InstanceLifecycle,
    /// Connected player entity IDs (fixed 4-slot co-op party array).
    pub party_members: [Option<u32>; MAX_PARTY_MEMBERS],
    /// Current count of active party members.
    pub party_count: usize,
    /// Number of simulation ticks elapsed since room allocation.
    pub elapsed_ticks: u32,
    /// Current health of the instance boss entity.
    pub boss_health: u32,
    /// Maximum initial health of the instance boss.
    pub boss_max_health: u32,
}

impl DungeonInstance {
    /// Creates a newly allocated dungeon instance.
    pub fn new(id: InstanceId, boss_max_health: u32) -> Self {
        Self {
            id,
            state: InstanceLifecycle::Allocated,
            party_members: [None; MAX_PARTY_MEMBERS],
            party_count: 0,
            elapsed_ticks: 0,
            boss_health: boss_max_health,
            boss_max_health,
        }
    }

    /// Adds a player entity to the co-op party.
    pub fn join_party(&mut self, player_id: u32) -> Result<(), WorldError> {
        if self.state == InstanceLifecycle::PendingCleanup {
            return Err(WorldError::InvalidInstanceState {
                expected: "Allocated or ActiveCombat",
                actual: "PendingCleanup",
            });
        }

        // Check for duplicate join
        if self.party_members.contains(&Some(player_id)) {
            return Ok(());
        }

        let slot = self
            .party_members
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(WorldError::PartyFull)?;

        *slot = Some(player_id);
        self.party_count += 1;

        // Automatically activate combat once at least one player enters
        if self.state == InstanceLifecycle::Allocated {
            self.state = InstanceLifecycle::ActiveCombat;
        }

        Ok(())
    }

    /// Removes a player entity from the co-op party.
    pub fn leave_party(&mut self, player_id: u32) -> Result<(), WorldError> {
        for slot in self.party_members.iter_mut() {
            if *slot == Some(player_id) {
                *slot = None;
                self.party_count = self.party_count.saturating_sub(1);

                // If room becomes empty, transition to cleanup
                if self.party_count == 0 {
                    self.state = InstanceLifecycle::PendingCleanup;
                }
                return Ok(());
            }
        }
        Err(WorldError::EntityNotFound(player_id))
    }

    /// Applies damage to the instance boss.
    ///
    /// Returns true if the boss was defeated and triggered the `VictoryReward` phase.
    pub fn apply_boss_damage(&mut self, damage: u32) -> bool {
        if self.state != InstanceLifecycle::ActiveCombat {
            return false;
        }

        self.boss_health = self.boss_health.saturating_sub(damage);
        if self.boss_health == 0 {
            self.state = InstanceLifecycle::VictoryReward;
            true
        } else {
            false
        }
    }

    /// Ticks the instance simulation clock.
    ///
    /// Returns true if the room is active, or false if it should be deallocated.
    pub fn tick(&mut self) -> bool {
        self.elapsed_ticks = self.elapsed_ticks.saturating_add(1);

        match self.state {
            InstanceLifecycle::Allocated => true,
            InstanceLifecycle::ActiveCombat => {
                // If combat runs for over 30 minutes (36,000 ticks), force timeout
                if self.elapsed_ticks > 36_000 {
                    self.state = InstanceLifecycle::PendingCleanup;
                }
                true
            }
            InstanceLifecycle::VictoryReward => {
                // Allow 60 seconds (1200 ticks) grace period for loot collection
                if self.elapsed_ticks > ROOM_CLEANUP_TIMEOUT_TICKS {
                    self.state = InstanceLifecycle::PendingCleanup;
                }
                true
            }
            InstanceLifecycle::PendingCleanup => false,
        }
    }
}

/// Pre-allocated memory pool of ephemeral dungeon rooms enabling scale-to-zero compute.
#[derive(Debug)]
pub struct DungeonPool<const MAX_ROOMS: usize> {
    rooms: [Option<DungeonInstance>; MAX_ROOMS],
    next_id: u64,
    active_count: usize,
}

impl<const MAX_ROOMS: usize> DungeonPool<MAX_ROOMS> {
    /// Creates a new empty `DungeonPool` with pre-allocated slot capacity.
    pub fn new() -> Self {
        Self {
            rooms: std::array::from_fn(|_| None),
            next_id: 1,
            active_count: 0,
        }
    }

    /// Allocates an ephemeral dungeon room from the pool in sub-microsecond time.
    ///
    /// Zero heap allocations occur during room acquisition.
    pub fn allocate_instance(&mut self, boss_max_health: u32) -> Result<InstanceId, WorldError> {
        let slot = self
            .rooms
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(WorldError::InstancePoolExhausted)?;

        let id = InstanceId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);

        *slot = Some(DungeonInstance::new(id, boss_max_health));
        self.active_count += 1;

        Ok(id)
    }

    /// Returns an immutable reference to an active instance room.
    pub fn get_instance(&self, id: InstanceId) -> Option<&DungeonInstance> {
        self.rooms
            .iter()
            .filter_map(|s| s.as_ref())
            .find(|room| room.id == id)
    }

    /// Returns a mutable reference to an active instance room.
    pub fn get_instance_mut(&mut self, id: InstanceId) -> Option<&mut DungeonInstance> {
        self.rooms
            .iter_mut()
            .filter_map(|s| s.as_mut())
            .find(|room| room.id == id)
    }

    /// Deallocates an ephemeral instance room and immediately recycles its memory slot.
    pub fn deallocate_instance(&mut self, id: InstanceId) -> Result<(), WorldError> {
        for slot in self.rooms.iter_mut() {
            if let Some(room) = slot {
                if room.id == id {
                    *slot = None;
                    self.active_count = self.active_count.saturating_sub(1);
                    return Ok(());
                }
            }
        }
        Err(WorldError::InstanceNotFound(id))
    }

    /// Returns the number of currently active dungeon rooms.
    #[inline]
    pub fn active_room_count(&self) -> usize {
        self.active_count
    }

    /// Ticks all active instances and purges rooms in the `PendingCleanup` state.
    pub fn tick_all(&mut self) {
        for slot in self.rooms.iter_mut() {
            let should_clean = if let Some(room) = slot {
                !room.tick()
            } else {
                false
            };

            if should_clean {
                *slot = None;
                self.active_count = self.active_count.saturating_sub(1);
            }
        }
    }
}

impl<const MAX_ROOMS: usize> Default for DungeonPool<MAX_ROOMS> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dungeon_instance_lifecycle_flow() {
        let mut room = DungeonInstance::new(InstanceId(1), 100);
        assert_eq!(room.state, InstanceLifecycle::Allocated);

        // Player 10 joins: transitions to ActiveCombat
        assert!(room.join_party(10).is_ok());
        assert_eq!(room.state, InstanceLifecycle::ActiveCombat);
        assert_eq!(room.party_count, 1);

        // Player 20 joins
        assert!(room.join_party(20).is_ok());
        assert_eq!(room.party_count, 2);

        // Damage boss
        assert!(!room.apply_boss_damage(60));
        assert_eq!(room.boss_health, 40);

        // Defeat boss: transitions to VictoryReward
        assert!(room.apply_boss_damage(40));
        assert_eq!(room.boss_health, 0);
        assert_eq!(room.state, InstanceLifecycle::VictoryReward);

        // Players leave: transitions to PendingCleanup
        assert!(room.leave_party(10).is_ok());
        assert!(room.leave_party(20).is_ok());
        assert_eq!(room.state, InstanceLifecycle::PendingCleanup);

        // Tick returns false indicating room ready for cleanup
        assert!(!room.tick());
    }

    #[test]
    fn test_dungeon_pool_zero_leak_lifecycle() {
        let mut pool = DungeonPool::<4>::new();
        assert_eq!(pool.active_room_count(), 0);

        let r1 = pool.allocate_instance(100).expect("allocate r1");
        let r2 = pool.allocate_instance(200).expect("allocate r2");
        assert_eq!(pool.active_room_count(), 2);

        // Deallocate r1
        assert!(pool.deallocate_instance(r1).is_ok());
        assert_eq!(pool.active_room_count(), 1);

        // Deallocate r2
        assert!(pool.deallocate_instance(r2).is_ok());
        assert_eq!(pool.active_room_count(), 0);
    }
}
