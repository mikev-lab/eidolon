//! Player equipment management and combat stat resolution.
//!
//! Enforces server-authoritative equipment slot rules, item stat aggregation,
//! and atomic item transitions between character bags and worn gear.

use eidolon_core::item::{get_item_definition, EquipmentSlot, StatBlock, NUM_EQUIPMENT_SLOTS};

use crate::error::WorldError;

/// Container holding equipped gear across the 9 standard equipment slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EquipmentContainer {
    /// Array of equipped item blueprint IDs indexed by `EquipmentSlot as usize`.
    pub slots: [Option<u32>; NUM_EQUIPMENT_SLOTS],
}

impl EquipmentContainer {
    /// Creates a new empty equipment container.
    pub const fn new() -> Self {
        Self {
            slots: [None; NUM_EQUIPMENT_SLOTS],
        }
    }

    /// Returns the item ID equipped in the specified slot, if any.
    #[inline]
    pub fn get(&self, slot: EquipmentSlot) -> Option<u32> {
        let idx = slot.as_u8() as usize;
        if idx < NUM_EQUIPMENT_SLOTS {
            self.slots[idx]
        } else {
            None
        }
    }

    /// Equips an item into the specified slot, returning the previously equipped item if any.
    ///
    /// Validates that the item blueprint exists and is eligible for the requested slot.
    pub fn equip(&mut self, slot: EquipmentSlot, item_id: u32) -> Result<Option<u32>, WorldError> {
        let def = get_item_definition(item_id).ok_or(WorldError::ItemNotFound(item_id))?;

        let allowed_slot = def
            .equip_slot
            .ok_or(WorldError::TransactionAborted("Item cannot be equipped"))?;

        // Handle Ring1 and Ring2 interchangeable compatibility
        let is_compatible = match (slot, allowed_slot) {
            (EquipmentSlot::Ring1, EquipmentSlot::Ring1)
            | (EquipmentSlot::Ring1, EquipmentSlot::Ring2)
            | (EquipmentSlot::Ring2, EquipmentSlot::Ring1)
            | (EquipmentSlot::Ring2, EquipmentSlot::Ring2) => true,
            (a, b) => a == b,
        };

        if !is_compatible {
            return Err(WorldError::TransactionAborted(
                "Incompatible equipment slot",
            ));
        }

        let idx = slot.as_u8() as usize;
        let prev = self.slots[idx];
        self.slots[idx] = Some(item_id);
        Ok(prev)
    }

    /// Unequips an item from the specified slot, returning the removed item ID.
    pub fn unequip(&mut self, slot: EquipmentSlot) -> Result<Option<u32>, WorldError> {
        let idx = slot.as_u8() as usize;
        if idx >= NUM_EQUIPMENT_SLOTS {
            return Err(WorldError::TransactionAborted("Invalid equipment slot"));
        }

        let prev = self.slots[idx].take();
        Ok(prev)
    }

    /// Computes the aggregate stat bonuses provided by all currently equipped gear.
    pub fn compute_stats(&self) -> StatBlock {
        let mut total = StatBlock::ZERO;
        for item_id in self.slots.iter().flatten() {
            if let Some(def) = get_item_definition(*item_id) {
                total = total.saturating_add(def.stats);
            }
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_equipment_lifecycle_and_stats() {
        let mut equipment = EquipmentContainer::new();
        assert_eq!(equipment.compute_stats(), StatBlock::ZERO);

        // Equip Longsword (+25 attack power) into MainHand
        let prev = equipment
            .equip(EquipmentSlot::MainHand, 1001)
            .expect("Equip sword");
        assert_eq!(prev, None);
        assert_eq!(equipment.get(EquipmentSlot::MainHand), Some(1001));

        let stats_1 = equipment.compute_stats();
        assert_eq!(stats_1.attack_power, 25);
        assert_eq!(stats_1.armor, 0);

        // Equip Kite Shield (+30 armor, +50 HP, -5 speed) into OffHand
        let prev_shield = equipment
            .equip(EquipmentSlot::OffHand, 1002)
            .expect("Equip shield");
        assert_eq!(prev_shield, None);

        let stats_2 = equipment.compute_stats();
        assert_eq!(stats_2.attack_power, 25);
        assert_eq!(stats_2.armor, 30);
        assert_eq!(stats_2.health_bonus, 50);
        assert_eq!(stats_2.speed_bonus, -5);

        // Unequip sword
        let unequipped = equipment
            .unequip(EquipmentSlot::MainHand)
            .expect("Unequip sword");
        assert_eq!(unequipped, Some(1001));
        assert_eq!(equipment.get(EquipmentSlot::MainHand), None);

        let stats_3 = equipment.compute_stats();
        assert_eq!(stats_3.attack_power, 0);
        assert_eq!(stats_3.armor, 30);
    }

    #[test]
    fn test_incompatible_slot_rejection() {
        let mut equipment = EquipmentContainer::new();
        // Try to equip shield into Head slot
        let res = equipment.equip(EquipmentSlot::Head, 1002);
        assert!(res.is_err());
    }

    #[test]
    fn test_ring_slot_interchangeability() {
        let mut equipment = EquipmentContainer::new();
        // Ring of Greater Power (defined as Ring1) can also be equipped in Ring2
        let res1 = equipment.equip(EquipmentSlot::Ring1, 3001);
        assert!(res1.is_ok());

        let res2 = equipment.equip(EquipmentSlot::Ring2, 3001);
        assert!(res2.is_ok());
    }
}
