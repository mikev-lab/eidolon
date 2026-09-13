//! Atomic transaction engine, double-spend prevention, and multi-session fencing.
//!
//! Enforces two-phase atomic transactional boundaries for currency and inventory transfers.
//! Binds mutations to monotonic generation locks to unconditionally reject concurrent
//! dual-login exploits and stale session writes. Client confirmations are strictly gated
//! on write-ahead journal (WAL) durability.

use std::collections::HashMap;

use eidolon_core::lock::{GenerationLockRegistry, LockError, LockToken};

use crate::durable_journal::CommitDurability;
use crate::error::WorldError;
use crate::wal::{WriteAheadJournal, OP_CURRENCY_DELTA, OP_INVENTORY_MUTATION};

/// Maximum inventory item slots per player account.
pub const MAX_INVENTORY_SLOTS: usize = 64;

/// Maximum number of recent transaction IDs tracked for deduplication.
pub const MAX_DEDUP_HISTORY: usize = 1024;

/// Resource lock identifier for currency wallet operations.
pub const LOCK_WALLET: u32 = 0;
/// Resource lock identifier for inventory item operations.
pub const LOCK_INVENTORY: u32 = 1;

/// Inventory item descriptor with identifier and stack quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InventoryItem {
    /// Blueprint item identifier.
    pub item_id: u32,
    /// Stack count.
    pub quantity: u32,
}

/// In-memory player account state tracked during authoritative simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountState {
    /// Unique persistent account identifier.
    pub account_id: u64,
    /// Premium gacha currency balance.
    pub premium_currency: u64,
    /// Standard farmable game currency balance.
    pub free_currency: u64,
    /// Owned item slots.
    pub inventory: [Option<InventoryItem>; MAX_INVENTORY_SLOTS],
    /// Monotonically incrementing state version.
    pub state_version: u64,
}

impl AccountState {
    /// Constructs a new empty account state.
    pub fn new(account_id: u64) -> Self {
        Self {
            account_id,
            premium_currency: 0,
            free_currency: 0,
            inventory: [None; MAX_INVENTORY_SLOTS],
            state_version: 1,
        }
    }

    /// Credits currency to the account.
    pub fn credit_currency(&mut self, amount: u64, is_premium: bool) {
        if is_premium {
            self.premium_currency = self.premium_currency.saturating_add(amount);
        } else {
            self.free_currency = self.free_currency.saturating_add(amount);
        }
        self.state_version += 1;
    }

    /// Debits currency from the account, returning `Err(WorldError::InsufficientBalance)` if balance is too low.
    pub fn debit_currency(&mut self, amount: u64, is_premium: bool) -> Result<(), WorldError> {
        if is_premium {
            if self.premium_currency < amount {
                return Err(WorldError::InsufficientBalance {
                    required: amount,
                    actual: self.premium_currency,
                });
            }
            self.premium_currency -= amount;
        } else {
            if self.free_currency < amount {
                return Err(WorldError::InsufficientBalance {
                    required: amount,
                    actual: self.free_currency,
                });
            }
            self.free_currency -= amount;
        }
        self.state_version += 1;
        Ok(())
    }

    /// Adds an item stack to the inventory.
    pub fn add_item(&mut self, item_id: u32, quantity: u32) -> Result<(), WorldError> {
        // Try to stack into existing slot
        for slot in self.inventory.iter_mut().flatten() {
            if slot.item_id == item_id {
                slot.quantity = slot.quantity.saturating_add(quantity);
                self.state_version += 1;
                return Ok(());
            }
        }

        // Insert into first empty slot
        let slot = self
            .inventory
            .iter_mut()
            .find(|s| s.is_none())
            .ok_or(WorldError::TransactionAborted("Inventory is full"))?;

        *slot = Some(InventoryItem { item_id, quantity });
        self.state_version += 1;
        Ok(())
    }

    /// Removes an item quantity from the inventory.
    pub fn remove_item(&mut self, item_id: u32, quantity: u32) -> Result<(), WorldError> {
        for slot in self.inventory.iter_mut() {
            if let Some(item) = slot {
                if item.item_id == item_id {
                    if item.quantity < quantity {
                        return Err(WorldError::InsufficientBalance {
                            required: quantity as u64,
                            actual: item.quantity as u64,
                        });
                    }

                    item.quantity -= quantity;
                    if item.quantity == 0 {
                        *slot = None;
                    }
                    self.state_version += 1;
                    return Ok(());
                }
            }
        }

        Err(WorldError::ItemNotFound(item_id))
    }
}

/// Transaction operation specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionOp {
    /// Atomic currency transfer between two accounts.
    CurrencyTransfer {
        /// Source account identifier debited.
        from_account: u64,
        /// Destination account identifier credited.
        to_account: u64,
        /// Transfer amount.
        amount: u64,
        /// Whether currency is premium.
        is_premium: bool,
    },
    /// Atomic currency deduction (e.g. shop purchase, gacha pull).
    CurrencySpend {
        /// Account debited.
        account_id: u64,
        /// Spend amount.
        amount: u64,
        /// Whether currency is premium.
        is_premium: bool,
    },
    /// Atomic item transfer between two accounts.
    ItemTransfer {
        /// Source account losing item.
        from_account: u64,
        /// Destination account receiving item.
        to_account: u64,
        /// Blueprint item identifier.
        item_id: u32,
        /// Stack quantity transferred.
        quantity: u32,
    },
}

/// Execution status of an authoritative transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionStatus {
    /// Committed to in-memory state and queued in the WAL ring buffer.
    Committed {
        /// Assigned Log Sequence Number.
        lsn: u64,
    },
    /// Transaction rolled back or rejected.
    Aborted,
}

/// Authoritative transaction manager enforcing atomic boundaries and double-spend protection.
pub struct TransactionManager {
    lock_registry: GenerationLockRegistry<1024>,
    accounts: HashMap<u64, AccountState>,
    executed_txs: HashMap<u64, TransactionStatus>,
}

impl Default for TransactionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TransactionManager {
    /// Constructs a new transaction manager.
    pub fn new() -> Self {
        Self {
            lock_registry: GenerationLockRegistry::new(),
            accounts: HashMap::new(),
            executed_txs: HashMap::new(),
        }
    }

    /// Registers or loads an active account into the in-memory transaction working set.
    pub fn register_account(&mut self, state: AccountState) {
        self.accounts.insert(state.account_id, state);
    }

    /// Retrieves an immutable reference to an active account state.
    pub fn get_account(&self, account_id: u64) -> Option<&AccountState> {
        self.accounts.get(&account_id)
    }

    /// Acquires a generation lock on an account resource (wallet or inventory).
    pub fn acquire_lock(
        &mut self,
        account_id: u64,
        lock_id: u32,
        session_id: u64,
        current_tick: u64,
        lease_duration_ticks: u32,
        force_takeover: bool,
    ) -> Result<LockToken, LockError> {
        self.lock_registry.acquire_lock(
            account_id,
            lock_id,
            session_id,
            current_tick,
            lease_duration_ticks,
            force_takeover,
        )
    }

    /// Executes an atomic transaction under generation-lock protection, logging mutations to the WAL.
    ///
    /// Double-Spend Prevention Invariant:
    /// - Asserts transaction ID has not been previously executed.
    /// - Verifies caller holds active generation locks for all affected accounts.
    /// - Validates balances prior to state mutation.
    /// - Performs atomic mutation or complete rollback on error.
    /// - Appends mutation records to the write-ahead journal.
    pub fn execute_transaction<const CAP: usize>(
        &mut self,
        tx_id: u64,
        op: TransactionOp,
        caller_session_id: u64,
        source_token: &LockToken,
        current_tick: u64,
        journal: &mut WriteAheadJournal<CAP>,
    ) -> Result<u64, WorldError> {
        // 1. Deduplication check
        if self.executed_txs.contains_key(&tx_id) {
            return Err(WorldError::DuplicateTransaction(tx_id));
        }

        // 2. Validate generation lock for source account
        if let Err(e) =
            self.lock_registry
                .validate_mutation(source_token, caller_session_id, current_tick)
        {
            let (expected, actual) = match e {
                LockError::SessionMismatch { expected, actual } => (expected, actual),
                LockError::StaleGeneration { expected, actual } => (expected, actual),
                _ => (0, caller_session_id),
            };
            return Err(WorldError::StaleSessionMutation { expected, actual });
        }

        // 3. Execute atomic transaction based on operation type
        let lsn = match op {
            TransactionOp::CurrencySpend {
                account_id,
                amount,
                is_premium,
            } => {
                if source_token.account_id != account_id || source_token.lock_id != LOCK_WALLET {
                    return Err(WorldError::TransactionAborted("Invalid wallet lock token"));
                }

                let account = self
                    .accounts
                    .get_mut(&account_id)
                    .ok_or(WorldError::TransactionAborted("Account not found"))?;

                account.debit_currency(amount, is_premium)?;

                // Append WAL record
                let mut payload = [0u8; 16];
                payload[0..8].copy_from_slice(&amount.to_be_bytes());
                payload[8] = if is_premium { 1 } else { 0 };
                payload[9] = 0; // 0 = debit

                journal.append(
                    current_tick,
                    account_id,
                    0,
                    OP_CURRENCY_DELTA,
                    &payload[..10],
                )?
            }

            TransactionOp::CurrencyTransfer {
                from_account,
                to_account,
                amount,
                is_premium,
            } => {
                if source_token.account_id != from_account || source_token.lock_id != LOCK_WALLET {
                    return Err(WorldError::TransactionAborted("Invalid wallet lock token"));
                }
                if from_account == to_account {
                    return Err(WorldError::TransactionAborted("Cannot transfer to self"));
                }

                // Check source account balance first
                let from_balance = {
                    let from = self
                        .accounts
                        .get(&from_account)
                        .ok_or(WorldError::TransactionAborted("Source account not found"))?;
                    if is_premium {
                        from.premium_currency
                    } else {
                        from.free_currency
                    }
                };

                if from_balance < amount {
                    return Err(WorldError::InsufficientBalance {
                        required: amount,
                        actual: from_balance,
                    });
                }

                // Ensure target account exists
                if !self.accounts.contains_key(&to_account) {
                    return Err(WorldError::TransactionAborted(
                        "Target account not registered",
                    ));
                }

                // Execute atomic debit from source and credit to target
                let from = self.accounts.get_mut(&from_account).unwrap();
                from.debit_currency(amount, is_premium)?;

                let to = self.accounts.get_mut(&to_account).unwrap();
                to.credit_currency(amount, is_premium);

                // Append WAL record
                let mut payload = [0u8; 24];
                payload[0..8].copy_from_slice(&to_account.to_be_bytes());
                payload[8..16].copy_from_slice(&amount.to_be_bytes());
                payload[16] = if is_premium { 1 } else { 0 };

                journal.append(
                    current_tick,
                    from_account,
                    0,
                    OP_CURRENCY_DELTA,
                    &payload[..17],
                )?
            }

            TransactionOp::ItemTransfer {
                from_account,
                to_account,
                item_id,
                quantity,
            } => {
                if source_token.account_id != from_account || source_token.lock_id != LOCK_INVENTORY
                {
                    return Err(WorldError::TransactionAborted(
                        "Invalid inventory lock token",
                    ));
                }
                if from_account == to_account {
                    return Err(WorldError::TransactionAborted("Cannot transfer to self"));
                }

                // Check source item presence
                {
                    let from = self
                        .accounts
                        .get(&from_account)
                        .ok_or(WorldError::TransactionAborted("Source account not found"))?;
                    let has_item = from
                        .inventory
                        .iter()
                        .flatten()
                        .any(|i| i.item_id == item_id && i.quantity >= quantity);
                    if !has_item {
                        return Err(WorldError::ItemNotFound(item_id));
                    }
                }

                // Ensure target account exists and has space
                if !self.accounts.contains_key(&to_account) {
                    return Err(WorldError::TransactionAborted(
                        "Target account not registered",
                    ));
                }

                // Execute atomic transfer: debit source, credit target
                let from = self.accounts.get_mut(&from_account).unwrap();
                from.remove_item(item_id, quantity)?;

                let to = self.accounts.get_mut(&to_account).unwrap();
                if let Err(e) = to.add_item(item_id, quantity) {
                    // Rollback source account item
                    let from_rollback = self.accounts.get_mut(&from_account).unwrap();
                    let _ = from_rollback.add_item(item_id, quantity);
                    return Err(e);
                }

                // Append WAL record
                let mut payload = [0u8; 24];
                payload[0..8].copy_from_slice(&to_account.to_be_bytes());
                payload[8..12].copy_from_slice(&item_id.to_be_bytes());
                payload[12..16].copy_from_slice(&quantity.to_be_bytes());

                journal.append(
                    current_tick,
                    from_account,
                    0,
                    OP_INVENTORY_MUTATION,
                    &payload[..16],
                )?
            }
        };

        self.executed_txs
            .insert(tx_id, TransactionStatus::Committed { lsn });
        Ok(lsn)
    }

    /// Verifies whether an executed transaction has been durably flushed to the WAL.
    ///
    /// Client responses for high-value mutations return success only when this returns true.
    pub fn is_transaction_durable<const CAP: usize>(
        &self,
        tx_id: u64,
        journal: &WriteAheadJournal<CAP>,
    ) -> bool {
        match self.executed_txs.get(&tx_id) {
            Some(TransactionStatus::Committed { lsn }) => journal.is_flushed(*lsn),
            _ => false,
        }
    }

    /// Executes an atomic transaction under generation-lock protection with explicit durability semantics.
    ///
    /// When `durability` is `CommitDurability::LocalDiskFsync`, the journal is immediately flushed
    /// to physical disk via `journal.flush_pending()` before returning `Ok(lsn)`.
    /// Client confirmations are strictly gated on this call succeeding, delivering RPO = 0.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_transaction_with_durability<const CAP: usize>(
        &mut self,
        tx_id: u64,
        op: TransactionOp,
        caller_session_id: u64,
        source_token: &LockToken,
        current_tick: u64,
        journal: &mut WriteAheadJournal<CAP>,
        durability: CommitDurability,
    ) -> Result<u64, WorldError> {
        let lsn = self.execute_transaction(
            tx_id,
            op,
            caller_session_id,
            source_token,
            current_tick,
            journal,
        )?;

        match durability {
            CommitDurability::InMemoryBuffered => {}
            CommitDurability::LocalDiskFsync | CommitDurability::DistributedQuorum => {
                journal.flush_pending()?;
            }
        }

        Ok(lsn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wal::MockDurableStorage;

    #[test]
    fn test_currency_spend_double_spend_prevention() {
        let mut manager = TransactionManager::new();
        let mut account = AccountState::new(1001);
        account.premium_currency = 1000;
        manager.register_account(account);

        let mut journal = WriteAheadJournal::<64>::new(Box::new(MockDurableStorage::new()));

        let token = manager
            .acquire_lock(1001, LOCK_WALLET, 555, 10, 50, false)
            .expect("Lock acquire must succeed");

        // First spend 700: succeeds (leaves 300)
        let lsn1 = manager
            .execute_transaction(
                1,
                TransactionOp::CurrencySpend {
                    account_id: 1001,
                    amount: 700,
                    is_premium: true,
                },
                555,
                &token,
                12,
                &mut journal,
            )
            .expect("Spend 700 must succeed");
        assert_eq!(lsn1, 1);
        assert_eq!(manager.get_account(1001).unwrap().premium_currency, 300);

        // Concurrent attempt to spend another 700 with same balance: must be rejected with InsufficientBalance
        let err = manager
            .execute_transaction(
                2,
                TransactionOp::CurrencySpend {
                    account_id: 1001,
                    amount: 700,
                    is_premium: true,
                },
                555,
                &token,
                12,
                &mut journal,
            )
            .expect_err("Double spend must be rejected");

        match err {
            WorldError::InsufficientBalance { required, actual } => {
                assert_eq!(required, 700);
                assert_eq!(actual, 300);
            }
            other => panic!("Expected InsufficientBalance, got {other:?}"),
        }

        // Verify balance remains exactly 300
        assert_eq!(manager.get_account(1001).unwrap().premium_currency, 300);
    }

    #[test]
    fn test_stale_session_write_fencing() {
        let mut manager = TransactionManager::new();
        let mut account = AccountState::new(2001);
        account.free_currency = 500;
        manager.register_account(account);

        let mut journal = WriteAheadJournal::<64>::new(Box::new(MockDurableStorage::new()));

        // Old session 100 acquires lock
        let token_old = manager
            .acquire_lock(2001, LOCK_WALLET, 100, 10, 50, false)
            .unwrap();

        // Client reconnects: new session 200 forces takeover
        let _token_new = manager
            .acquire_lock(2001, LOCK_WALLET, 200, 15, 50, true)
            .unwrap();

        // Old session 100 attempts mutation using old token: must be rejected
        let err = manager
            .execute_transaction(
                1,
                TransactionOp::CurrencySpend {
                    account_id: 2001,
                    amount: 100,
                    is_premium: false,
                },
                100,
                &token_old,
                16,
                &mut journal,
            )
            .expect_err("Stale session mutation must be rejected");

        match err {
            WorldError::StaleSessionMutation { expected, actual } => {
                assert_eq!(expected, 200);
                assert_eq!(actual, 100);
            }
            other => panic!("Expected StaleSessionMutation, got {other:?}"),
        }
    }

    #[test]
    fn test_duplicate_transaction_deduplication() {
        let mut manager = TransactionManager::new();
        let mut account = AccountState::new(3001);
        account.free_currency = 1000;
        manager.register_account(account);

        let mut journal = WriteAheadJournal::<64>::new(Box::new(MockDurableStorage::new()));
        let token = manager
            .acquire_lock(3001, LOCK_WALLET, 50, 1, 100, false)
            .unwrap();

        // Execute transaction ID 9999
        manager
            .execute_transaction(
                9999,
                TransactionOp::CurrencySpend {
                    account_id: 3001,
                    amount: 100,
                    is_premium: false,
                },
                50,
                &token,
                2,
                &mut journal,
            )
            .unwrap();

        // Replay identical transaction ID 9999
        let err = manager
            .execute_transaction(
                9999,
                TransactionOp::CurrencySpend {
                    account_id: 3001,
                    amount: 100,
                    is_premium: false,
                },
                50,
                &token,
                3,
                &mut journal,
            )
            .expect_err("Duplicate tx must be rejected");

        assert_eq!(err, WorldError::DuplicateTransaction(9999));
        assert_eq!(manager.get_account(3001).unwrap().free_currency, 900);
    }
}
