//! Monotonic generation-locked distributed tokens for double-spend prevention and session fencing.
//!
//! Enforces single-writer invariants across concurrent logins, transfers, and reconnects.
//! Ensures that stale or duplicate sessions cannot modify player inventory or currency state.

use core::fmt;

/// Maximum number of active locks tracked in a single fixed-capacity lock table.
pub const MAX_TRACKED_LOCKS: usize = 1024;

/// Errors arising from generation lock validation and acquisition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockError {
    /// Lock is currently held by another active session.
    Contention {
        /// Account identifier.
        account_id: u64,
        /// Lock identifier.
        lock_id: u32,
        /// Session identifier currently holding the lock.
        holder_session_id: u64,
        /// Remaining valid lease duration in ticks.
        remaining_ticks: u64,
    },
    /// The supplied generation counter is older than the active lock generation.
    StaleGeneration {
        /// Active generation on the server.
        expected: u64,
        /// Stale generation presented by the client or request.
        actual: u64,
    },
    /// The session identifier does not match the active lock holder.
    SessionMismatch {
        /// Active session holding the lock.
        expected: u64,
        /// Session attempting the mutation.
        actual: u64,
    },
    /// The lock lease has expired and must be re-acquired.
    LeaseExpired {
        /// Tick at which the lease expired.
        expired_at_tick: u64,
        /// Current simulation tick.
        current_tick: u64,
    },
    /// Lock table capacity has been exceeded.
    CapacityExceeded,
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contention {
                account_id,
                lock_id,
                holder_session_id,
                remaining_ticks,
            } => {
                write!(
                    f,
                    "Lock contention on account {account_id} lock {lock_id}: held by session {holder_session_id} ({remaining_ticks} ticks remaining)"
                )
            }
            Self::StaleGeneration { expected, actual } => {
                write!(
                    f,
                    "Stale generation token: expected {expected}, got {actual}"
                )
            }
            Self::SessionMismatch { expected, actual } => {
                write!(
                    f,
                    "Session mismatch on lock: expected {expected}, got {actual}"
                )
            }
            Self::LeaseExpired {
                expired_at_tick,
                current_tick,
            } => {
                write!(
                    f,
                    "Lock lease expired at tick {expired_at_tick} (current tick {current_tick})"
                )
            }
            Self::CapacityExceeded => write!(f, "Generation lock table capacity exceeded"),
        }
    }
}

impl std::error::Error for LockError {}

/// Immutable authorization token proving ownership of an authoritative lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockToken {
    /// Account identifier associated with the lock.
    pub account_id: u64,
    /// Specific resource lock identifier (e.g. 0 for inventory, 1 for wallet).
    pub lock_id: u32,
    /// Monotonically increasing generation sequence number.
    pub generation: u64,
    /// Session identifier holding this lock grant.
    pub holder_session_id: u64,
    /// Simulation tick at which the lock was acquired or renewed.
    pub granted_tick: u64,
    /// Lease duration in simulation ticks.
    pub lease_duration_ticks: u32,
}

impl LockToken {
    /// Checks whether the token lease is still active at the given simulation tick.
    #[inline]
    pub const fn is_valid_at(&self, current_tick: u64) -> bool {
        current_tick >= self.granted_tick
            && current_tick
                < self
                    .granted_tick
                    .saturating_add(self.lease_duration_ticks as u64)
    }

    /// Computes the expiry tick of this lock.
    #[inline]
    pub const fn expires_at(&self) -> u64 {
        self.granted_tick
            .saturating_add(self.lease_duration_ticks as u64)
    }
}

/// In-memory record tracking an active generation lock in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockEntry {
    /// Active lock token.
    pub token: LockToken,
    /// Whether this entry is in active use.
    pub is_occupied: bool,
}

impl Default for LockEntry {
    fn default() -> Self {
        Self {
            token: LockToken {
                account_id: 0,
                lock_id: 0,
                generation: 0,
                holder_session_id: 0,
                granted_tick: 0,
                lease_duration_ticks: 0,
            },
            is_occupied: false,
        }
    }
}

/// Fixed-capacity registry managing generation locks with zero dynamic allocations.
#[derive(Debug)]
pub struct GenerationLockRegistry<const CAP: usize = MAX_TRACKED_LOCKS> {
    entries: [LockEntry; CAP],
    count: usize,
}

impl<const CAP: usize> Default for GenerationLockRegistry<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAP: usize> GenerationLockRegistry<CAP> {
    /// Constructs a new empty generation lock registry.
    pub const fn new() -> Self {
        Self {
            entries: [LockEntry {
                token: LockToken {
                    account_id: 0,
                    lock_id: 0,
                    generation: 0,
                    holder_session_id: 0,
                    granted_tick: 0,
                    lease_duration_ticks: 0,
                },
                is_occupied: false,
            }; CAP],
            count: 0,
        }
    }

    /// Acquires or renews a generation lock for an account and resource.
    ///
    /// If an active lock exists:
    /// - If held by the same session, renews the lease without incrementing generation.
    /// - If held by a different session and not expired, returns `Err(LockError::Contention)`.
    /// - If held by a different session but expired (or forced via takeover), increments generation.
    pub fn acquire_lock(
        &mut self,
        account_id: u64,
        lock_id: u32,
        session_id: u64,
        current_tick: u64,
        lease_duration_ticks: u32,
        force_takeover: bool,
    ) -> Result<LockToken, LockError> {
        // Search for existing entry
        for slot in self.entries.iter_mut() {
            if slot.is_occupied
                && slot.token.account_id == account_id
                && slot.token.lock_id == lock_id
            {
                let is_valid = slot.token.is_valid_at(current_tick);

                if slot.token.holder_session_id == session_id {
                    // Same session: renew lease
                    slot.token.granted_tick = current_tick;
                    slot.token.lease_duration_ticks = lease_duration_ticks;
                    return Ok(slot.token);
                }

                if is_valid && !force_takeover {
                    let remaining = slot.token.expires_at().saturating_sub(current_tick);
                    return Err(LockError::Contention {
                        account_id,
                        lock_id,
                        holder_session_id: slot.token.holder_session_id,
                        remaining_ticks: remaining,
                    });
                }

                // Lock was either expired or preempted via force takeover: increment generation
                let next_gen = slot.token.generation.saturating_add(1);
                slot.token = LockToken {
                    account_id,
                    lock_id,
                    generation: next_gen,
                    holder_session_id: session_id,
                    granted_tick: current_tick,
                    lease_duration_ticks,
                };
                return Ok(slot.token);
            }
        }

        // Allocate a new entry
        for slot in self.entries.iter_mut() {
            if !slot.is_occupied {
                slot.is_occupied = true;
                slot.token = LockToken {
                    account_id,
                    lock_id,
                    generation: 1,
                    holder_session_id: session_id,
                    granted_tick: current_tick,
                    lease_duration_ticks,
                };
                self.count += 1;
                return Ok(slot.token);
            }
        }

        Err(LockError::CapacityExceeded)
    }

    /// Validates that a mutation request holds the current authoritative generation lock.
    pub fn validate_mutation(
        &self,
        token: &LockToken,
        session_id: u64,
        current_tick: u64,
    ) -> Result<(), LockError> {
        for slot in self.entries.iter() {
            if slot.is_occupied
                && slot.token.account_id == token.account_id
                && slot.token.lock_id == token.lock_id
            {
                if slot.token.holder_session_id != session_id {
                    return Err(LockError::SessionMismatch {
                        expected: slot.token.holder_session_id,
                        actual: session_id,
                    });
                }

                if slot.token.generation != token.generation {
                    return Err(LockError::StaleGeneration {
                        expected: slot.token.generation,
                        actual: token.generation,
                    });
                }

                if !slot.token.is_valid_at(current_tick) {
                    return Err(LockError::LeaseExpired {
                        expired_at_tick: slot.token.expires_at(),
                        current_tick,
                    });
                }

                return Ok(());
            }
        }

        // Lock not found in active registry
        Err(LockError::LeaseExpired {
            expired_at_tick: token.expires_at(),
            current_tick,
        })
    }

    /// Explicitly releases a held lock, freeing the slot in the registry.
    pub fn release_lock(
        &mut self,
        account_id: u64,
        lock_id: u32,
        session_id: u64,
        generation: u64,
    ) -> Result<(), LockError> {
        for slot in self.entries.iter_mut() {
            if slot.is_occupied
                && slot.token.account_id == account_id
                && slot.token.lock_id == lock_id
            {
                if slot.token.holder_session_id != session_id {
                    return Err(LockError::SessionMismatch {
                        expected: slot.token.holder_session_id,
                        actual: session_id,
                    });
                }

                if slot.token.generation != generation {
                    return Err(LockError::StaleGeneration {
                        expected: slot.token.generation,
                        actual: generation,
                    });
                }

                slot.is_occupied = false;
                self.count = self.count.saturating_sub(1);
                return Ok(());
            }
        }

        Ok(())
    }

    /// Returns the number of active locks currently tracked.
    #[inline]
    pub const fn active_count(&self) -> usize {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_acquire_and_renew_same_session() {
        let mut registry = GenerationLockRegistry::<16>::new();

        let token1 = registry
            .acquire_lock(1001, 0, 999, 100, 20, false)
            .expect("First acquisition must succeed");
        assert_eq!(token1.generation, 1);
        assert_eq!(token1.holder_session_id, 999);
        assert!(token1.is_valid_at(110));

        // Renewal by same session at tick 115
        let token2 = registry
            .acquire_lock(1001, 0, 999, 115, 20, false)
            .expect("Renewal must succeed");
        assert_eq!(token2.generation, 1); // Generation unchanged on renewal
        assert_eq!(token2.granted_tick, 115);
        assert!(token2.is_valid_at(130));
    }

    #[test]
    fn test_lock_contention_blocks_dual_session() {
        let mut registry = GenerationLockRegistry::<16>::new();

        let _token1 = registry
            .acquire_lock(1001, 0, 999, 100, 20, false)
            .expect("First session acquires");

        // Second session attempts acquisition while first is still valid
        let err = registry
            .acquire_lock(1001, 0, 888, 105, 20, false)
            .expect_err("Second session must be rejected");

        match err {
            LockError::Contention {
                holder_session_id,
                remaining_ticks,
                ..
            } => {
                assert_eq!(holder_session_id, 999);
                assert_eq!(remaining_ticks, 15);
            }
            other => panic!("Expected Contention error, got {other:?}"),
        }
    }

    #[test]
    fn test_force_takeover_increments_generation() {
        let mut registry = GenerationLockRegistry::<16>::new();

        let token1 = registry
            .acquire_lock(1001, 0, 999, 100, 20, false)
            .expect("First session acquires");
        assert_eq!(token1.generation, 1);

        // Newer reconnect forces takeover
        let token2 = registry
            .acquire_lock(1001, 0, 888, 105, 20, true)
            .expect("Force takeover must succeed");
        assert_eq!(token2.generation, 2);
        assert_eq!(token2.holder_session_id, 888);

        // Old token validation fails
        let val_err = registry
            .validate_mutation(&token1, 999, 110)
            .expect_err("Old session token must be invalid");
        assert_eq!(
            val_err,
            LockError::SessionMismatch {
                expected: 888,
                actual: 999
            }
        );
    }
}
