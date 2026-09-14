//! Two-Phase Commit (2PC) Atomic Item Transaction Coordinator.
//!
//! Enforces ACID atomic transfers, slot lock fencing, and double-spend prevention
//! across the Universal Container Graph (inventories, nested bags, chests, bank vaults).

use std::collections::HashMap;

use eidolon_core::{Container, ContainerError, ITEM_FLAG_SOULBOUND};

/// Maximum transaction history entries retained for idempotency deduplication.
pub const MAX_TX_DEDUP_CAPACITY: usize = 2048;

/// Errors arising during item transaction execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemTransactionError {
    /// Source container does not exist.
    SourceContainerNotFound,
    /// Target container does not exist.
    TargetContainerNotFound,
    /// Item not found in source slot.
    ItemNotFound,
    /// Source or target slot is locked by an in-flight transaction.
    SlotLocked,
    /// Requested transfer quantity exceeds item stack count.
    InsufficientQuantity,
    /// Target container has no remaining empty slots.
    TargetContainerFull,
    /// Target container would exceed maximum weight limit.
    TargetWeightLimitExceeded,
    /// Soulbound item cannot be traded to a different owner.
    SoulboundItemUntransferable,
    /// Recursive container nesting cycle detected.
    RecursiveNestingNotAllowed,
    /// Transaction ID has already been executed (duplicate request).
    DuplicateTransactionId,
    /// Target slot is already occupied.
    TargetSlotNotEmpty,
    /// Item type mismatch during stack merge.
    ItemTypeMismatch,
}

impl core::fmt::Display for ItemTransactionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SourceContainerNotFound => write!(f, "Source container not found"),
            Self::TargetContainerNotFound => write!(f, "Target container not found"),
            Self::ItemNotFound => write!(f, "Item not found in source slot"),
            Self::SlotLocked => write!(f, "Slot locked by in-flight transaction"),
            Self::InsufficientQuantity => write!(f, "Insufficient item quantity"),
            Self::TargetContainerFull => write!(f, "Target container is full"),
            Self::TargetWeightLimitExceeded => write!(f, "Target weight limit exceeded"),
            Self::SoulboundItemUntransferable => {
                write!(f, "Soulbound item cannot be transferred to another owner")
            }
            Self::RecursiveNestingNotAllowed => write!(f, "Cannot nest container inside itself"),
            Self::DuplicateTransactionId => write!(f, "Duplicate transaction ID"),
            Self::TargetSlotNotEmpty => write!(f, "Target slot is not empty"),
            Self::ItemTypeMismatch => write!(f, "Item type mismatch during stack merge"),
        }
    }
}

impl std::error::Error for ItemTransactionError {}

impl From<ContainerError> for ItemTransactionError {
    fn from(err: ContainerError) -> Self {
        match err {
            ContainerError::SlotLocked => Self::SlotLocked,
            ContainerError::CapacityFull => Self::TargetContainerFull,
            ContainerError::WeightLimitExceeded => Self::TargetWeightLimitExceeded,
            ContainerError::SlotNotEmpty => Self::TargetSlotNotEmpty,
            ContainerError::ItemTypeMismatch => Self::ItemTypeMismatch,
            ContainerError::RecursiveNestingNotAllowed => Self::RecursiveNestingNotAllowed,
            _ => Self::ItemNotFound,
        }
    }
}

/// Intent descriptor for an atomic item transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemTransferIntent {
    /// Unique 64-bit idempotency transaction identifier.
    pub transaction_id: u64,
    /// Source container ID.
    pub source_container_id: u64,
    /// Source slot index.
    pub source_slot: u16,
    /// Expected item instance ID.
    pub item_instance_id: u64,
    /// Target container ID.
    pub target_container_id: u64,
    /// Target slot index (None = first available slot).
    pub target_slot: Option<u16>,
    /// Quantity of items to transfer.
    pub quantity: u32,
    /// Estimated or authoritative item weight in grams.
    pub item_weight_grams: u32,
}

/// Result of a successfully committed item transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemTransferResult {
    /// Transaction ID.
    pub transaction_id: u64,
    /// Source container ID.
    pub source_container_id: u64,
    /// Target container ID.
    pub target_container_id: u64,
    /// Resulting target slot index.
    pub target_slot: u16,
    /// Transferred item instance ID.
    pub item_instance_id: u64,
    /// Transferred quantity.
    pub quantity: u32,
}

/// Authoritative coordinator managing containers and 2PC item transfers.
#[derive(Debug)]
pub struct ItemTransactionCoordinator {
    containers: HashMap<u64, Container>,
    processed_transactions: [u64; MAX_TX_DEDUP_CAPACITY],
    dedup_cursor: usize,
    next_instance_id: u64,
}

impl Default for ItemTransactionCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl ItemTransactionCoordinator {
    /// Creates a new item transaction coordinator.
    pub fn new() -> Self {
        Self {
            containers: HashMap::new(),
            processed_transactions: [0; MAX_TX_DEDUP_CAPACITY],
            dedup_cursor: 0,
            next_instance_id: 100_000,
        }
    }

    /// Allocates a new unique item instance ID.
    pub fn allocate_instance_id(&mut self) -> u64 {
        let id = self.next_instance_id;
        self.next_instance_id = self.next_instance_id.saturating_add(1);
        id
    }

    /// Registers a container with the coordinator.
    pub fn register_container(&mut self, container: Container) {
        self.containers.insert(container.container_id, container);
    }

    /// Retrieves an immutable reference to a container.
    pub fn get_container(&self, container_id: u64) -> Option<&Container> {
        self.containers.get(&container_id)
    }

    /// Retrieves a mutable reference to a container.
    pub fn get_container_mut(&mut self, container_id: u64) -> Option<&mut Container> {
        self.containers.get_mut(&container_id)
    }

    /// Checks if a transaction ID has already been executed.
    pub fn is_transaction_processed(&self, tx_id: u64) -> bool {
        self.processed_transactions.contains(&tx_id)
    }

    /// Records a transaction ID into the deduplication ring buffer.
    fn record_transaction(&mut self, tx_id: u64) {
        self.processed_transactions[self.dedup_cursor] = tx_id;
        self.dedup_cursor = (self.dedup_cursor + 1) % MAX_TX_DEDUP_CAPACITY;
    }

    /// Executes an atomic Two-Phase Commit item transfer between containers.
    ///
    /// Phase 1 (Prepare): Validates ownership, locks slots, asserts capacities.
    /// Phase 2 (Commit): Mutates containers, generates split instances if needed, unlocks slots.
    /// On any failure: Automatically releases all locks without altering state.
    pub fn execute_transfer(
        &mut self,
        intent: ItemTransferIntent,
    ) -> Result<ItemTransferResult, ItemTransactionError> {
        // 0. Deduplication check
        if self.is_transaction_processed(intent.transaction_id) {
            return Err(ItemTransactionError::DuplicateTransactionId);
        }

        // 1. Phase 1 (Prepare & Validate)
        // Check source container
        let (src_owner, is_same_container) = {
            let src = self
                .containers
                .get(&intent.source_container_id)
                .ok_or(ItemTransactionError::SourceContainerNotFound)?;
            (
                src.owner_id,
                intent.source_container_id == intent.target_container_id,
            )
        };

        // Check target container
        let dst_owner = {
            let dst = self
                .containers
                .get(&intent.target_container_id)
                .ok_or(ItemTransactionError::TargetContainerNotFound)?;
            dst.owner_id
        };

        // Validate source slot item
        {
            let src = self.containers.get(&intent.source_container_id).unwrap();
            let slot = src
                .get_slot(intent.source_slot)
                .ok_or(ItemTransactionError::ItemNotFound)?;

            if slot.is_locked {
                return Err(ItemTransactionError::SlotLocked);
            }

            let item = slot
                .item
                .as_ref()
                .ok_or(ItemTransactionError::ItemNotFound)?;
            if item.instance_id != intent.item_instance_id {
                return Err(ItemTransactionError::ItemNotFound);
            }

            if intent.quantity == 0 || intent.quantity > item.quantity {
                return Err(ItemTransactionError::InsufficientQuantity);
            }

            // Soulbound check: cannot trade to different owner
            if (item.flags & ITEM_FLAG_SOULBOUND) != 0 && src_owner != dst_owner {
                return Err(ItemTransactionError::SoulboundItemUntransferable);
            }

            // Recursive nesting check if transferring a container
            if let Some(nested_id) = item.nested_container_id {
                if nested_id == intent.target_container_id {
                    return Err(ItemTransactionError::RecursiveNestingNotAllowed);
                }
            }
        }

        // Validate target container capacity & weight
        {
            let dst = self.containers.get(&intent.target_container_id).unwrap();
            if dst.max_weight_grams > 0 {
                let projected_weight = dst
                    .current_weight_grams
                    .saturating_add(intent.item_weight_grams);
                if projected_weight > dst.max_weight_grams {
                    return Err(ItemTransactionError::TargetWeightLimitExceeded);
                }
            }

            if let Some(target_slot) = intent.target_slot {
                let slot = dst
                    .get_slot(target_slot)
                    .ok_or(ItemTransactionError::TargetSlotNotEmpty)?;
                if slot.is_locked {
                    return Err(ItemTransactionError::SlotLocked);
                }
                if slot.item.is_some() {
                    return Err(ItemTransactionError::TargetSlotNotEmpty);
                }
            } else if dst.free_slot_count() == 0 {
                // If moving inside same container and taking the entire slot, slot count remains net-neutral
                if !is_same_container {
                    return Err(ItemTransactionError::TargetContainerFull);
                }
            }
        }

        // Lock source slot
        {
            let src = self
                .containers
                .get_mut(&intent.source_container_id)
                .unwrap();
            src.lock_slot(intent.source_slot)?;
        }

        // 2. Phase 2 (Commit & Execute)
        let transfer_result = self.commit_transfer(intent);

        // Unlock source slot regardless of commit outcome
        if let Some(src) = self.containers.get_mut(&intent.source_container_id) {
            src.unlock_slot(intent.source_slot);
        }

        match transfer_result {
            Ok(res) => {
                self.record_transaction(intent.transaction_id);
                Ok(res)
            }
            Err(e) => Err(e),
        }
    }

    /// Internal commit phase executing the actual in-memory mutation.
    fn commit_transfer(
        &mut self,
        intent: ItemTransferIntent,
    ) -> Result<ItemTransferResult, ItemTransactionError> {
        let is_same_container = intent.source_container_id == intent.target_container_id;
        let new_split_id = self.allocate_instance_id();

        if is_same_container {
            // Intra-container move/split
            let src = self
                .containers
                .get_mut(&intent.source_container_id)
                .unwrap();

            let full_stack = {
                let item = src
                    .get_slot(intent.source_slot)
                    .unwrap()
                    .item
                    .as_ref()
                    .unwrap();
                item.quantity == intent.quantity
            };

            let item_to_move = if full_stack {
                src.commit_remove_item(intent.source_slot, 0)?
            } else {
                src.commit_split_stack(intent.source_slot, intent.quantity, new_split_id)?
            };

            let resulting_slot = src.insert_item(item_to_move, intent.target_slot, 0)?;

            Ok(ItemTransferResult {
                transaction_id: intent.transaction_id,
                source_container_id: intent.source_container_id,
                target_container_id: intent.target_container_id,
                target_slot: resulting_slot,
                item_instance_id: intent.item_instance_id,
                quantity: intent.quantity,
            })
        } else {
            // Inter-container transfer
            let full_stack = {
                let src = self.containers.get(&intent.source_container_id).unwrap();
                let item = src
                    .get_slot(intent.source_slot)
                    .unwrap()
                    .item
                    .as_ref()
                    .unwrap();
                item.quantity == intent.quantity
            };

            let item_to_move = {
                let src = self
                    .containers
                    .get_mut(&intent.source_container_id)
                    .unwrap();
                if full_stack {
                    src.commit_remove_item(intent.source_slot, intent.item_weight_grams)?
                } else {
                    src.commit_split_stack(intent.source_slot, intent.quantity, new_split_id)?
                }
            };

            let dst = self
                .containers
                .get_mut(&intent.target_container_id)
                .unwrap();
            let resulting_slot = match dst.insert_item(
                item_to_move.clone(),
                intent.target_slot,
                intent.item_weight_grams,
            ) {
                Ok(slot) => slot,
                Err(e) => {
                    // Rollback source item if destination insert failed
                    let src = self
                        .containers
                        .get_mut(&intent.source_container_id)
                        .unwrap();
                    let _ = src.insert_item(
                        item_to_move,
                        Some(intent.source_slot),
                        intent.item_weight_grams,
                    );
                    return Err(e.into());
                }
            };

            Ok(ItemTransferResult {
                transaction_id: intent.transaction_id,
                source_container_id: intent.source_container_id,
                target_container_id: intent.target_container_id,
                target_slot: resulting_slot,
                item_instance_id: intent.item_instance_id,
                quantity: intent.quantity,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eidolon_core::{ContainerType, ItemInstance};

    #[test]
    fn test_2pc_item_transfer_happy_path() {
        let mut coordinator = ItemTransactionCoordinator::new();

        let mut player_bag = Container::new(1, 100, ContainerType::PlayerInventory, 16, 10000);
        let sword = ItemInstance::new(500, 1001, 1, 100);
        player_bag.insert_item(sword, Some(0), 1500).unwrap();

        let chest = Container::new(2, 100, ContainerType::StructureChest, 32, 50000);

        coordinator.register_container(player_bag);
        coordinator.register_container(chest);

        let intent = ItemTransferIntent {
            transaction_id: 1,
            source_container_id: 1,
            source_slot: 0,
            item_instance_id: 500,
            target_container_id: 2,
            target_slot: Some(5),
            quantity: 1,
            item_weight_grams: 1500,
        };

        let res = coordinator.execute_transfer(intent).unwrap();
        assert_eq!(res.target_slot, 5);

        let p_bag = coordinator.get_container(1).unwrap();
        assert_eq!(p_bag.occupied_count(), 0);
        assert_eq!(p_bag.current_weight_grams, 0);

        let c_chest = coordinator.get_container(2).unwrap();
        assert_eq!(c_chest.occupied_count(), 1);
        assert_eq!(c_chest.current_weight_grams, 1500);
        assert_eq!(
            c_chest
                .get_slot(5)
                .unwrap()
                .item
                .as_ref()
                .unwrap()
                .instance_id,
            500
        );
    }

    #[test]
    fn test_2pc_soulbound_rejection() {
        let mut coordinator = ItemTransactionCoordinator::new();

        let mut p1_bag = Container::new(1, 101, ContainerType::PlayerInventory, 16, 0);
        let mut bound_sword = ItemInstance::new(501, 1001, 1, 100);
        bound_sword.flags |= ITEM_FLAG_SOULBOUND;
        p1_bag.insert_item(bound_sword, Some(0), 0).unwrap();

        let p2_bag = Container::new(2, 202, ContainerType::PlayerInventory, 16, 0);

        coordinator.register_container(p1_bag);
        coordinator.register_container(p2_bag);

        let intent = ItemTransferIntent {
            transaction_id: 2,
            source_container_id: 1,
            source_slot: 0,
            item_instance_id: 501,
            target_container_id: 2,
            target_slot: None,
            quantity: 1,
            item_weight_grams: 0,
        };

        let err = coordinator.execute_transfer(intent);
        assert_eq!(err, Err(ItemTransactionError::SoulboundItemUntransferable));

        // Source item remains untouched
        let p1 = coordinator.get_container(1).unwrap();
        assert_eq!(p1.occupied_count(), 1);
    }

    #[test]
    fn test_duplicate_transaction_id_rejected() {
        let mut coordinator = ItemTransactionCoordinator::new();

        let mut bag = Container::new(1, 100, ContainerType::PlayerInventory, 16, 0);
        let potions = ItemInstance::new(600, 2001, 10, 0);
        bag.insert_item(potions, Some(0), 0).unwrap();

        let vault = Container::new(2, 100, ContainerType::BankVault, 16, 0);

        coordinator.register_container(bag);
        coordinator.register_container(vault);

        let intent = ItemTransferIntent {
            transaction_id: 99,
            source_container_id: 1,
            source_slot: 0,
            item_instance_id: 600,
            target_container_id: 2,
            target_slot: None,
            quantity: 5,
            item_weight_grams: 0,
        };

        assert!(coordinator.execute_transfer(intent).is_ok());

        // Duplicate replay
        assert_eq!(
            coordinator.execute_transfer(intent),
            Err(ItemTransactionError::DuplicateTransactionId)
        );
    }
}
