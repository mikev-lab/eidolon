//! Authoritative Threat and Aggro Table Management for NPCs.
//!
//! Provides tank taunt locking, threat decay, and target switching hysteresis
//! (110% melee / 130% ranged) to eliminate visual target flickering.

/// An entry in an NPC's threat table representing an active combatant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreatEntry {
    /// Entity ID of the attacker.
    pub entity_id: u64,
    /// Accumulated threat score.
    pub threat_points: u32,
    /// Cumulative damage dealt to this NPC.
    pub damage_dealt: u32,
    /// Cumulative healing generated while in combat with this NPC.
    pub healing_done: u32,
    /// Tick when this attacker last generated threat.
    pub last_threat_tick: u64,
}

impl ThreatEntry {
    /// Creates a new threat entry for an attacker.
    pub const fn new(entity_id: u64, initial_threat: u32, current_tick: u64) -> Self {
        Self {
            entity_id,
            threat_points: initial_threat,
            damage_dealt: 0,
            healing_done: 0,
            last_threat_tick: current_tick,
        }
    }
}

/// Fixed-capacity threat table supporting up to 16 concurrent combatants with zero heap allocations.
#[derive(Debug, Clone, Default)]
pub struct ThreatTable {
    entries: [Option<ThreatEntry>; 16],
    count: usize,
    taunt_target: Option<u64>,
    taunt_expire_tick: u64,
}

impl ThreatTable {
    /// Creates an empty threat table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of tracked combatants.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Checks if the threat table is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Adds threat points for an attacker.
    pub fn add_threat(
        &mut self,
        entity_id: u64,
        threat_amount: u32,
        is_damage: bool,
        current_tick: u64,
    ) {
        // Look for existing entry
        for slot in self.entries.iter_mut().flatten() {
            if slot.entity_id == entity_id {
                slot.threat_points = slot.threat_points.saturating_add(threat_amount);
                if is_damage {
                    slot.damage_dealt = slot.damage_dealt.saturating_add(threat_amount);
                } else {
                    slot.healing_done = slot.healing_done.saturating_add(threat_amount);
                }
                slot.last_threat_tick = current_tick;
                return;
            }
        }

        // Insert into first empty slot
        for slot in self.entries.iter_mut() {
            if slot.is_none() {
                let mut entry = ThreatEntry::new(entity_id, threat_amount, current_tick);
                if is_damage {
                    entry.damage_dealt = threat_amount;
                } else {
                    entry.healing_done = threat_amount;
                }
                *slot = Some(entry);
                self.count += 1;
                return;
            }
        }

        // If table full, replace the slot with lowest threat if new threat is higher
        let mut lowest_idx = None;
        let mut lowest_threat = u32::MAX;

        for (idx, slot) in self.entries.iter().enumerate() {
            if let Some(entry) = slot {
                if entry.threat_points < lowest_threat {
                    lowest_threat = entry.threat_points;
                    lowest_idx = Some(idx);
                }
            }
        }

        if let Some(idx) = lowest_idx {
            if threat_amount > lowest_threat {
                let mut entry = ThreatEntry::new(entity_id, threat_amount, current_tick);
                if is_damage {
                    entry.damage_dealt = threat_amount;
                } else {
                    entry.healing_done = threat_amount;
                }
                self.entries[idx] = Some(entry);
            }
        }
    }

    /// Forces the NPC to target a specific combatant for a set duration (Taunt).
    pub fn apply_taunt(&mut self, entity_id: u64, duration_ticks: u64, current_tick: u64) {
        // Ensure the taunting entity exists in the threat table if not already present
        if !self
            .entries
            .iter()
            .flatten()
            .any(|e| e.entity_id == entity_id)
        {
            self.add_threat(entity_id, 1, false, current_tick);
        }
        self.taunt_target = Some(entity_id);
        self.taunt_expire_tick = current_tick.saturating_add(duration_ticks);
    }

    /// Returns the entity ID and threat points of the highest-threat combatant.
    pub fn get_top_threat(&self) -> Option<(u64, u32)> {
        let mut top: Option<(u64, u32)> = None;

        for slot in self.entries.iter().flatten() {
            match top {
                Some((_, max_threat)) => {
                    if slot.threat_points > max_threat {
                        top = Some((slot.entity_id, slot.threat_points));
                    }
                }
                None => {
                    top = Some((slot.entity_id, slot.threat_points));
                }
            }
        }

        top
    }

    /// Evaluates and selects the primary attack target with hysteresis.
    ///
    /// Hysteresis: Switching away from the current target requires a competitor
    /// to exceed the current target's threat by:
    /// - Melee range: 110% (threshold = current_threat * 11 / 10)
    /// - Ranged: 130% (threshold = current_threat * 13 / 10)
    pub fn select_target(
        &self,
        current_target: Option<u64>,
        is_melee: bool,
        current_tick: u64,
    ) -> Option<u64> {
        // Taunt lock takes absolute precedence while active
        if let Some(taunt_id) = self.taunt_target {
            if current_tick < self.taunt_expire_tick {
                return Some(taunt_id);
            }
        }

        let (top_id, top_threat) = self.get_top_threat()?;

        // If no current target or current target is no longer in table, switch to top
        let curr_id = match current_target {
            Some(id) => id,
            None => return Some(top_id),
        };

        if curr_id == top_id {
            return Some(curr_id);
        }

        // Find current target threat
        let curr_threat = self
            .entries
            .iter()
            .flatten()
            .find(|e| e.entity_id == curr_id)
            .map(|e| e.threat_points);

        match curr_threat {
            Some(ct) => {
                // Apply hysteresis threshold
                let threshold = if is_melee {
                    // 110% requirement: ct + ct / 10
                    ct.saturating_add(ct / 10)
                } else {
                    // 130% requirement: ct + (ct * 3) / 10
                    ct.saturating_add((ct.saturating_mul(3)) / 10)
                };

                if top_threat >= threshold {
                    Some(top_id) // Switched target!
                } else {
                    Some(curr_id) // Hysteresis kept target!
                }
            }
            None => Some(top_id), // Current target vanished from table
        }
    }

    /// Removes an entity from the threat table (e.g. died, vanished, feigned death).
    pub fn remove_entity(&mut self, entity_id: u64) {
        for slot in self.entries.iter_mut() {
            if let Some(entry) = slot {
                if entry.entity_id == entity_id {
                    *slot = None;
                    self.count = self.count.saturating_sub(1);
                    break;
                }
            }
        }

        if self.taunt_target == Some(entity_id) {
            self.taunt_target = None;
        }
    }

    /// Decays threat across all combatants by a percentage factor (0..100).
    pub fn decay_threat(&mut self, decay_percent: u32) {
        let pct = decay_percent.min(100);
        let factor = 100 - pct;

        for slot in self.entries.iter_mut().flatten() {
            slot.threat_points = (slot.threat_points as u64 * factor as u64 / 100) as u32;
        }
    }

    /// Purges combatants who have not generated threat within `timeout_ticks`.
    pub fn decay_stale_threat(&mut self, timeout_ticks: u64, current_tick: u64) {
        for slot in self.entries.iter_mut() {
            if let Some(entry) = slot {
                if current_tick.saturating_sub(entry.last_threat_tick) > timeout_ticks {
                    *slot = None;
                    self.count = self.count.saturating_sub(1);
                }
            }
        }
    }

    /// Clears all entries from the threat table (e.g. upon entering Evade/Reset state).
    pub fn clear(&mut self) {
        self.entries = [None; 16];
        self.count = 0;
        self.taunt_target = None;
        self.taunt_expire_tick = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_threat_table_insertion_and_top_threat() {
        let mut table = ThreatTable::new();
        table.add_threat(101, 500, true, 1);
        table.add_threat(102, 800, true, 1);
        table.add_threat(103, 300, true, 1);

        assert_eq!(table.len(), 3);
        let (top_id, top_threat) = table.get_top_threat().unwrap();
        assert_eq!(top_id, 102);
        assert_eq!(top_threat, 800);
    }

    #[test]
    fn test_target_switching_hysteresis_melee() {
        let mut table = ThreatTable::new();
        table.add_threat(101, 1000, true, 1); // Current target with 1000 threat
        table.add_threat(102, 1050, true, 1); // Competitor with 1050 threat (105% of 1000)

        // In melee, 110% is required (1100). Competitor has 1050 -> Keep current target!
        let target = table.select_target(Some(101), true, 1);
        assert_eq!(target, Some(101));

        // Now competitor reaches 1120 threat (exceeds 110% = 1100) -> Switch target!
        table.add_threat(102, 70, true, 2); // 1050 + 70 = 1120
        let target_switched = table.select_target(Some(101), true, 2);
        assert_eq!(target_switched, Some(102));
    }

    #[test]
    fn test_taunt_lock_overrides_threat() {
        let mut table = ThreatTable::new();
        table.add_threat(101, 5000, true, 1); // DPS has 5000 threat
        table.add_threat(102, 1000, true, 1); // Tank has 1000 threat

        // Tank taunts for 5 ticks
        table.apply_taunt(102, 5, 1);

        // At tick 3, taunt lock is active -> Target must be tank (102)
        assert_eq!(table.select_target(Some(101), true, 3), Some(102));

        // At tick 7, taunt expired -> Re-evaluates top threat (DPS 101)
        assert_eq!(table.select_target(Some(102), true, 7), Some(101));
    }

    #[test]
    fn test_clear_resets_threat() {
        let mut table = ThreatTable::new();
        table.add_threat(101, 1000, true, 1);
        table.clear();
        assert!(table.is_empty());
        assert_eq!(table.get_top_threat(), None);
    }
}
