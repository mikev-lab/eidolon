//! Universal Transactional Container & Item Graph (The EVE/SWG Economic Substrate).
//!
//! Provides globally unique item instances with durability, crafter attribution provenance,
//! nested containers (bags inside bags, cargo crates, chests), and bounded slot containers.

/// Flag bit indicating an item is soulbound to its owner.
pub const ITEM_FLAG_SOULBOUND: u16 = 1 << 0;
/// Flag bit indicating an item has sustained durability damage.
pub const ITEM_FLAG_DAMAGED: u16 = 1 << 1;
/// Flag bit indicating an item is locked in an active transaction.
pub const ITEM_FLAG_TRANSACTION_LOCKED: u16 = 1 << 2;
/// Flag bit indicating an item functions as a nested container.
pub const ITEM_FLAG_CONTAINER: u16 = 1 << 3;

/// Errors arising from container slot and item manipulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerError {
    /// Target slot index is out of bounds for the container.
    SlotOutOfRange,
    /// Target slot is already occupied by an item.
    SlotNotEmpty,
    /// Specified slot is empty.
    SlotEmpty,
    /// Target slot is locked in an active transaction.
    SlotLocked,
    /// Container has reached maximum slot capacity.
    CapacityFull,
    /// Operation exceeds the container's weight limit.
    WeightLimitExceeded,
    /// Cannot merge items of differing type IDs.
    ItemTypeMismatch,
    /// Stack operation exceeds maximum stack size.
    StackLimitExceeded,
    /// Invalid quantity specified (e.g. 0 or greater than stack size).
    InvalidQuantity,
    /// Item instance was not found in container.
    ItemNotFound,
    /// Self-nesting cycle detected (container inside itself).
    RecursiveNestingNotAllowed,
}

impl core::fmt::Display for ContainerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SlotOutOfRange => write!(f, "Slot index out of range"),
            Self::SlotNotEmpty => write!(f, "Slot is not empty"),
            Self::SlotEmpty => write!(f, "Slot is empty"),
            Self::SlotLocked => write!(f, "Slot is locked in active transaction"),
            Self::CapacityFull => write!(f, "Container capacity is full"),
            Self::WeightLimitExceeded => write!(f, "Container weight limit exceeded"),
            Self::ItemTypeMismatch => write!(f, "Cannot merge items of differing types"),
            Self::StackLimitExceeded => write!(f, "Stack limit exceeded"),
            Self::InvalidQuantity => write!(f, "Invalid item quantity"),
            Self::ItemNotFound => write!(f, "Item instance not found"),
            Self::RecursiveNestingNotAllowed => {
                write!(f, "Recursive container nesting not allowed")
            }
        }
    }
}

impl std::error::Error for ContainerError {}

/// Unique item instance in the authoritative world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemInstance {
    /// Globally unique item instance ID (64-bit UUID).
    pub instance_id: u64,
    /// Item blueprint definition ID.
    pub type_id: u32,
    /// Current stacked count.
    pub quantity: u32,
    /// Current durability points.
    pub durability: u16,
    /// Maximum durability points.
    pub max_durability: u16,
    /// Item state bitmask flags.
    pub flags: u16,
    /// Crafter or manufacturer account ID (provenance).
    pub creator_id: Option<u64>,
    /// Nested container ID if this item is a bag, chest, or crate.
    pub nested_container_id: Option<u64>,
}

impl ItemInstance {
    /// Creates a new pristine item instance.
    pub fn new(instance_id: u64, type_id: u32, quantity: u32, max_durability: u16) -> Self {
        Self {
            instance_id,
            type_id,
            quantity: quantity.max(1),
            durability: max_durability,
            max_durability,
            flags: 0,
            creator_id: None,
            nested_container_id: None,
        }
    }

    /// Creates a crafted item instance with crafter attribution.
    pub fn new_crafted(
        instance_id: u64,
        type_id: u32,
        quantity: u32,
        max_durability: u16,
        creator_id: u64,
    ) -> Self {
        Self {
            instance_id,
            type_id,
            quantity: quantity.max(1),
            durability: max_durability,
            max_durability,
            flags: 0,
            creator_id: Some(creator_id),
            nested_container_id: None,
        }
    }

    /// Creates a bag or crate item instance that wraps a nested container.
    pub fn new_container(
        instance_id: u64,
        type_id: u32,
        max_durability: u16,
        nested_container_id: u64,
    ) -> Self {
        Self {
            instance_id,
            type_id,
            quantity: 1,
            durability: max_durability,
            max_durability,
            flags: ITEM_FLAG_CONTAINER,
            creator_id: None,
            nested_container_id: Some(nested_container_id),
        }
    }

    /// Applies durability wear. Returns true if durability reached zero (broken).
    pub fn apply_wear(&mut self, wear_points: u16) -> bool {
        self.durability = self.durability.saturating_sub(wear_points);
        if self.durability == 0 {
            self.flags |= ITEM_FLAG_DAMAGED;
            true
        } else {
            false
        }
    }

    /// Repairs durability by a specified amount.
    pub fn repair(&mut self, repair_points: u16) {
        self.durability = (self.durability.saturating_add(repair_points)).min(self.max_durability);
        if self.durability > 0 {
            self.flags &= !ITEM_FLAG_DAMAGED;
        }
    }

    /// Checks if the item is broken (0 durability).
    pub fn is_broken(&self) -> bool {
        self.durability == 0
    }
}

/// Container classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerType {
    /// Primary character backpack / inventory.
    PlayerInventory,
    /// Wearable equipment container.
    PlayerEquipment,
    /// Nested bag or pouch inside inventory.
    NestedBag,
    /// Long-term persistent bank or vault.
    BankVault,
    /// Grounded house or guild chest.
    StructureChest,
    /// Player market vendor kiosk or merchant inventory.
    VendorKiosk,
    /// Ephemeral defeated mob corpse loot container.
    CorpseLoot,
}

/// Single slot within a container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSlot {
    /// Zero-based slot index.
    pub slot_index: u16,
    /// Item occupant, if any.
    pub item: Option<ItemInstance>,
    /// Whether this slot is locked for 2PC transaction processing.
    pub is_locked: bool,
}

impl ContainerSlot {
    /// Creates an empty unlocked container slot.
    pub fn new(slot_index: u16) -> Self {
        Self {
            slot_index,
            item: None,
            is_locked: false,
        }
    }
}

/// Bounded transactional item container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    /// Unique container identifier.
    pub container_id: u64,
    /// Owning entity or account ID.
    pub owner_id: u64,
    /// Container functional type.
    pub container_type: ContainerType,
    /// Maximum number of item slots.
    pub capacity: u16,
    /// Maximum total weight in grams (0 = unlimited).
    pub max_weight_grams: u32,
    /// Current aggregated weight in grams.
    pub current_weight_grams: u32,
    /// Array of container slots.
    pub slots: Vec<ContainerSlot>,
}

impl Container {
    /// Creates a new container with the specified capacity.
    pub fn new(
        container_id: u64,
        owner_id: u64,
        container_type: ContainerType,
        capacity: u16,
        max_weight_grams: u32,
    ) -> Self {
        let mut slots = Vec::with_capacity(capacity as usize);
        for i in 0..capacity {
            slots.push(ContainerSlot::new(i));
        }

        Self {
            container_id,
            owner_id,
            container_type,
            capacity,
            max_weight_grams,
            current_weight_grams: 0,
            slots,
        }
    }

    /// Returns the number of occupied slots.
    pub fn occupied_count(&self) -> usize {
        self.slots.iter().filter(|s| s.item.is_some()).count()
    }

    /// Returns the number of available empty slots.
    pub fn free_slot_count(&self) -> usize {
        self.slots.iter().filter(|s| s.item.is_none()).count()
    }

    /// Finds the index of the first empty unlocked slot.
    pub fn find_first_empty_slot(&self) -> Option<u16> {
        self.slots
            .iter()
            .find(|s| s.item.is_none() && !s.is_locked)
            .map(|s| s.slot_index)
    }

    /// Retrieves a reference to a slot by index.
    pub fn get_slot(&self, slot_idx: u16) -> Option<&ContainerSlot> {
        self.slots.get(slot_idx as usize)
    }

    /// Retrieves a mutable reference to a slot by index.
    pub fn get_slot_mut(&mut self, slot_idx: u16) -> Option<&mut ContainerSlot> {
        self.slots.get_mut(slot_idx as usize)
    }

    /// Finds an item instance by its 64-bit UUID.
    pub fn find_item(&self, instance_id: u64) -> Option<(u16, &ItemInstance)> {
        for slot in &self.slots {
            if let Some(ref item) = slot.item {
                if item.instance_id == instance_id {
                    return Some((slot.slot_index, item));
                }
            }
        }
        None
    }

    /// Locks a slot for two-phase transaction isolation.
    pub fn lock_slot(&mut self, slot_idx: u16) -> Result<(), ContainerError> {
        let slot = self
            .slots
            .get_mut(slot_idx as usize)
            .ok_or(ContainerError::SlotOutOfRange)?;
        if slot.is_locked {
            return Err(ContainerError::SlotLocked);
        }
        slot.is_locked = true;
        Ok(())
    }

    /// Unlocks a slot after transaction completion or abort.
    pub fn unlock_slot(&mut self, slot_idx: u16) {
        if let Some(slot) = self.slots.get_mut(slot_idx as usize) {
            slot.is_locked = false;
        }
    }

    /// Inserts an item into a specified slot or the first available empty slot.
    pub fn insert_item(
        &mut self,
        item: ItemInstance,
        target_slot: Option<u16>,
        item_weight_grams: u32,
    ) -> Result<u16, ContainerError> {
        // Prevent self-nesting if item is a container
        if let Some(nested_id) = item.nested_container_id {
            if nested_id == self.container_id {
                return Err(ContainerError::RecursiveNestingNotAllowed);
            }
        }

        // Weight verification
        if self.max_weight_grams > 0 {
            let new_weight = self.current_weight_grams.saturating_add(item_weight_grams);
            if new_weight > self.max_weight_grams {
                return Err(ContainerError::WeightLimitExceeded);
            }
        }

        let slot_idx = match target_slot {
            Some(idx) => {
                let slot = self
                    .slots
                    .get(idx as usize)
                    .ok_or(ContainerError::SlotOutOfRange)?;
                if slot.is_locked {
                    return Err(ContainerError::SlotLocked);
                }
                if slot.item.is_some() {
                    return Err(ContainerError::SlotNotEmpty);
                }
                idx
            }
            None => self
                .find_first_empty_slot()
                .ok_or(ContainerError::CapacityFull)?,
        };

        let slot = &mut self.slots[slot_idx as usize];
        slot.item = Some(item);
        self.current_weight_grams = self.current_weight_grams.saturating_add(item_weight_grams);

        Ok(slot_idx)
    }

    /// Removes an item from the specified slot.
    pub fn remove_item(
        &mut self,
        slot_idx: u16,
        item_weight_grams: u32,
    ) -> Result<ItemInstance, ContainerError> {
        let slot = self
            .slots
            .get_mut(slot_idx as usize)
            .ok_or(ContainerError::SlotOutOfRange)?;
        if slot.is_locked {
            return Err(ContainerError::SlotLocked);
        }

        let item = slot.item.take().ok_or(ContainerError::SlotEmpty)?;
        self.current_weight_grams = self.current_weight_grams.saturating_sub(item_weight_grams);

        Ok(item)
    }

    /// Splits a stack of items in a slot, producing a new ItemInstance.
    pub fn split_stack(
        &mut self,
        slot_idx: u16,
        split_amount: u32,
        new_instance_id: u64,
    ) -> Result<ItemInstance, ContainerError> {
        if split_amount == 0 {
            return Err(ContainerError::InvalidQuantity);
        }

        let slot = self
            .slots
            .get_mut(slot_idx as usize)
            .ok_or(ContainerError::SlotOutOfRange)?;
        if slot.is_locked {
            return Err(ContainerError::SlotLocked);
        }

        let item = slot.item.as_mut().ok_or(ContainerError::SlotEmpty)?;
        if split_amount >= item.quantity {
            return Err(ContainerError::InvalidQuantity);
        }

        item.quantity -= split_amount;

        let mut new_item = item.clone();
        new_item.instance_id = new_instance_id;
        new_item.quantity = split_amount;

        Ok(new_item)
    }

    /// Removes an item from the specified slot, clearing any active transaction lock.
    pub fn commit_remove_item(
        &mut self,
        slot_idx: u16,
        item_weight_grams: u32,
    ) -> Result<ItemInstance, ContainerError> {
        self.unlock_slot(slot_idx);
        self.remove_item(slot_idx, item_weight_grams)
    }

    /// Splits a stack from the specified slot, clearing any active transaction lock.
    pub fn commit_split_stack(
        &mut self,
        slot_idx: u16,
        split_amount: u32,
        new_instance_id: u64,
    ) -> Result<ItemInstance, ContainerError> {
        self.unlock_slot(slot_idx);
        self.split_stack(slot_idx, split_amount, new_instance_id)
    }

    /// Merges items from one slot into another, adhering to the stack limit.
    pub fn merge_stack(
        &mut self,
        from_slot: u16,
        to_slot: u16,
        stack_limit: u32,
    ) -> Result<(), ContainerError> {
        if from_slot == to_slot {
            return Ok(());
        }

        let (from_item, to_item) = {
            let from_s = self
                .slots
                .get(from_slot as usize)
                .ok_or(ContainerError::SlotOutOfRange)?;
            let to_s = self
                .slots
                .get(to_slot as usize)
                .ok_or(ContainerError::SlotOutOfRange)?;

            if from_s.is_locked || to_s.is_locked {
                return Err(ContainerError::SlotLocked);
            }

            let f_item = from_s.item.as_ref().ok_or(ContainerError::SlotEmpty)?;
            let t_item = to_s.item.as_ref().ok_or(ContainerError::SlotEmpty)?;

            if f_item.type_id != t_item.type_id {
                return Err(ContainerError::ItemTypeMismatch);
            }

            (f_item.clone(), t_item.clone())
        };

        let available_space = stack_limit.saturating_sub(to_item.quantity);
        if available_space == 0 {
            return Err(ContainerError::StackLimitExceeded);
        }

        let transfer_amount = from_item.quantity.min(available_space);

        if from_item.quantity == transfer_amount {
            // Source slot becomes empty
            self.slots[from_slot as usize].item = None;
        } else {
            // Partial merge
            if let Some(ref mut f) = self.slots[from_slot as usize].item {
                f.quantity -= transfer_amount;
            }
        }

        if let Some(ref mut t) = self.slots[to_slot as usize].item {
            t.quantity += transfer_amount;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_slot_insertion_and_removal() {
        let mut container = Container::new(100, 1, ContainerType::PlayerInventory, 16, 5000);
        assert_eq!(container.free_slot_count(), 16);
        assert_eq!(container.occupied_count(), 0);

        let item = ItemInstance::new(5001, 1001, 1, 100);
        let slot = container.insert_item(item, Some(0), 500).unwrap();
        assert_eq!(slot, 0);
        assert_eq!(container.occupied_count(), 1);
        assert_eq!(container.current_weight_grams, 500);

        // Remove item
        let removed = container.remove_item(0, 500).unwrap();
        assert_eq!(removed.instance_id, 5001);
        assert_eq!(container.occupied_count(), 0);
        assert_eq!(container.current_weight_grams, 0);
    }

    #[test]
    fn test_stack_splitting_and_merging() {
        let mut container = Container::new(101, 1, ContainerType::PlayerInventory, 8, 0);
        let item = ItemInstance::new(6001, 2001, 20, 0);
        container.insert_item(item, Some(0), 0).unwrap();

        // Split 5 from slot 0
        let split_item = container.split_stack(0, 5, 6002).unwrap();
        assert_eq!(split_item.quantity, 5);
        assert_eq!(
            container
                .get_slot(0)
                .unwrap()
                .item
                .as_ref()
                .unwrap()
                .quantity,
            15
        );

        // Put split item into slot 1
        container.insert_item(split_item, Some(1), 0).unwrap();

        // Merge back: slot 1 into slot 0 with max stack 50
        container.merge_stack(1, 0, 50).unwrap();
        assert_eq!(
            container
                .get_slot(0)
                .unwrap()
                .item
                .as_ref()
                .unwrap()
                .quantity,
            20
        );
        assert!(container.get_slot(1).unwrap().item.is_none());
    }

    #[test]
    fn test_weight_limit_enforcement() {
        let mut container = Container::new(102, 1, ContainerType::PlayerInventory, 4, 1000);
        let item1 = ItemInstance::new(7001, 3001, 1, 100);
        container.insert_item(item1, None, 600).unwrap();

        let item2 = ItemInstance::new(7002, 3002, 1, 100);
        // Exceeds 1000g (600 + 500 = 1100)
        let res = container.insert_item(item2, None, 500);
        assert_eq!(res, Err(ContainerError::WeightLimitExceeded));
    }

    #[test]
    fn test_crafter_provenance_and_wear() {
        let mut item = ItemInstance::new_crafted(8001, 4001, 1, 100, 9999);
        assert_eq!(item.creator_id, Some(9999));
        assert_eq!(item.durability, 100);

        assert!(!item.apply_wear(40));
        assert_eq!(item.durability, 60);

        // Break item
        assert!(item.apply_wear(60));
        assert_eq!(item.durability, 0);
        assert!(item.is_broken());

        // Repair item
        item.repair(50);
        assert_eq!(item.durability, 50);
        assert!(!item.is_broken());
    }
}
