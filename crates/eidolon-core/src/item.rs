//! Item definitions, equipment slot topologies, and combat attribute stat blocks.
//!
//! Provides deterministic item structures and aggregate character stat arithmetic
//! shared between server validation and client prediction models.

/// Number of standard character equipment slots.
pub const NUM_EQUIPMENT_SLOTS: usize = 9;

/// Standard character equipment slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum EquipmentSlot {
    /// Head slot (helmets, hoods).
    Head = 0,
    /// Chest slot (cuirasses, robes).
    Chest = 1,
    /// Primary weapon hand (swords, daggers, staves).
    MainHand = 2,
    /// Secondary hand (shields, off-hand tomes).
    OffHand = 3,
    /// Legs slot (greaves, pants).
    Legs = 4,
    /// Feet slot (boots, treads).
    Feet = 5,
    /// First ring slot.
    Ring1 = 6,
    /// Second ring slot.
    Ring2 = 7,
    /// Neck slot (amulets, pendants).
    Amulet = 8,
}

impl EquipmentSlot {
    /// Attempts to parse an equipment slot from a discrete u8 index.
    pub const fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Head),
            1 => Some(Self::Chest),
            2 => Some(Self::MainHand),
            3 => Some(Self::OffHand),
            4 => Some(Self::Legs),
            5 => Some(Self::Feet),
            6 => Some(Self::Ring1),
            7 => Some(Self::Ring2),
            8 => Some(Self::Amulet),
            _ => None,
        }
    }

    /// Converts equipment slot to its discrete u8 representation.
    #[inline]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Character combat attribute modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatBlock {
    /// Bonus maximum health points.
    pub health_bonus: u32,
    /// Bonus maximum mana / energy points.
    pub mana_bonus: u32,
    /// Attack power modifying weapon physical damage.
    pub attack_power: u32,
    /// Armor value reducing physical damage.
    pub armor: u32,
    /// Movement speed modifier (percentage delta, e.g. +10 = +10% speed).
    pub speed_bonus: i32,
}

impl StatBlock {
    /// Zeroed attribute block.
    pub const ZERO: Self = Self {
        health_bonus: 0,
        mana_bonus: 0,
        attack_power: 0,
        armor: 0,
        speed_bonus: 0,
    };

    /// Combines two stat blocks using saturating arithmetic.
    pub fn saturating_add(self, rhs: Self) -> Self {
        Self {
            health_bonus: self.health_bonus.saturating_add(rhs.health_bonus),
            mana_bonus: self.mana_bonus.saturating_add(rhs.mana_bonus),
            attack_power: self.attack_power.saturating_add(rhs.attack_power),
            armor: self.armor.saturating_add(rhs.armor),
            speed_bonus: self.speed_bonus.saturating_add(rhs.speed_bonus),
        }
    }
}

/// Static blueprint item definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemDefinition {
    /// Unique item blueprint identifier.
    pub item_id: u32,
    /// Display name.
    pub name: &'static str,
    /// Associated equipment slot if wearable, or None for consumables/materials.
    pub equip_slot: Option<EquipmentSlot>,
    /// Maximum stack capacity in a single inventory slot.
    pub stack_limit: u32,
    /// Stat bonuses granted when equipped.
    pub stats: StatBlock,
    /// Required character level to equip or consume.
    pub required_level: u32,
}

/// Returns the blueprint definition for standard built-in items.
pub fn get_item_definition(item_id: u32) -> Option<ItemDefinition> {
    match item_id {
        // Weapons
        1001 => Some(ItemDefinition {
            item_id: 1001,
            name: "Steel Longsword",
            equip_slot: Some(EquipmentSlot::MainHand),
            stack_limit: 1,
            stats: StatBlock {
                health_bonus: 0,
                mana_bonus: 0,
                attack_power: 25,
                armor: 0,
                speed_bonus: 0,
            },
            required_level: 1,
        }),
        1002 => Some(ItemDefinition {
            item_id: 1002,
            name: "Iron Kite Shield",
            equip_slot: Some(EquipmentSlot::OffHand),
            stack_limit: 1,
            stats: StatBlock {
                health_bonus: 50,
                mana_bonus: 0,
                attack_power: 0,
                armor: 30,
                speed_bonus: -5,
            },
            required_level: 1,
        }),

        // Armor
        2001 => Some(ItemDefinition {
            item_id: 2001,
            name: "Knight's Great Helm",
            equip_slot: Some(EquipmentSlot::Head),
            stack_limit: 1,
            stats: StatBlock {
                health_bonus: 40,
                mana_bonus: 0,
                attack_power: 0,
                armor: 20,
                speed_bonus: 0,
            },
            required_level: 1,
        }),
        2002 => Some(ItemDefinition {
            item_id: 2002,
            name: "Paladin's Plate Cuirass",
            equip_slot: Some(EquipmentSlot::Chest),
            stack_limit: 1,
            stats: StatBlock {
                health_bonus: 100,
                mana_bonus: 20,
                attack_power: 5,
                armor: 50,
                speed_bonus: 0,
            },
            required_level: 1,
        }),
        2003 => Some(ItemDefinition {
            item_id: 2003,
            name: "Reinforced Greaves",
            equip_slot: Some(EquipmentSlot::Legs),
            stack_limit: 1,
            stats: StatBlock {
                health_bonus: 35,
                mana_bonus: 0,
                attack_power: 0,
                armor: 25,
                speed_bonus: 0,
            },
            required_level: 1,
        }),
        2004 => Some(ItemDefinition {
            item_id: 2004,
            name: "Boots of Swiftness",
            equip_slot: Some(EquipmentSlot::Feet),
            stack_limit: 1,
            stats: StatBlock {
                health_bonus: 15,
                mana_bonus: 0,
                attack_power: 0,
                armor: 10,
                speed_bonus: 15,
            },
            required_level: 1,
        }),

        // Accessories
        3001 => Some(ItemDefinition {
            item_id: 3001,
            name: "Ring of Greater Power",
            equip_slot: Some(EquipmentSlot::Ring1),
            stack_limit: 1,
            stats: StatBlock {
                health_bonus: 10,
                mana_bonus: 25,
                attack_power: 15,
                armor: 0,
                speed_bonus: 0,
            },
            required_level: 1,
        }),
        3002 => Some(ItemDefinition {
            item_id: 3002,
            name: "Amulet of Vitality",
            equip_slot: Some(EquipmentSlot::Amulet),
            stack_limit: 1,
            stats: StatBlock {
                health_bonus: 80,
                mana_bonus: 10,
                attack_power: 0,
                armor: 5,
                speed_bonus: 0,
            },
            required_level: 1,
        }),

        // Consumables
        4001 => Some(ItemDefinition {
            item_id: 4001,
            name: "Greater Health Potion",
            equip_slot: None,
            stack_limit: 20,
            stats: StatBlock::ZERO,
            required_level: 1,
        }),
        4002 => Some(ItemDefinition {
            item_id: 4002,
            name: "Greater Mana Potion",
            equip_slot: None,
            stack_limit: 20,
            stats: StatBlock::ZERO,
            required_level: 1,
        }),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_equipment_slot_roundtrip() {
        for i in 0..=8 {
            let slot = EquipmentSlot::from_u8(i).expect("Valid slot");
            assert_eq!(slot.as_u8(), i);
        }
        assert_eq!(EquipmentSlot::from_u8(9), None);
    }

    #[test]
    fn test_stat_block_saturating_add() {
        let a = StatBlock {
            health_bonus: 100,
            mana_bonus: 50,
            attack_power: 10,
            armor: 20,
            speed_bonus: 5,
        };
        let b = StatBlock {
            health_bonus: 50,
            mana_bonus: 30,
            attack_power: 15,
            armor: 10,
            speed_bonus: -2,
        };
        let c = a.saturating_add(b);

        assert_eq!(c.health_bonus, 150);
        assert_eq!(c.mana_bonus, 80);
        assert_eq!(c.attack_power, 25);
        assert_eq!(c.armor, 30);
        assert_eq!(c.speed_bonus, 3);
    }

    #[test]
    fn test_item_definitions() {
        let sword = get_item_definition(1001).expect("Sword exists");
        assert_eq!(sword.name, "Steel Longsword");
        assert_eq!(sword.equip_slot, Some(EquipmentSlot::MainHand));
        assert_eq!(sword.stats.attack_power, 25);

        let potion = get_item_definition(4001).expect("Potion exists");
        assert_eq!(potion.equip_slot, None);
        assert_eq!(potion.stack_limit, 20);
    }
}
