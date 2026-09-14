//! Autonomous NPC Ecosystem Manager with leashing, threat aggregation, and multi-clock simulation LOD.
//!
//! Evaluates behavior trees, aggro transitions, and spatial sensory perception
//! with zero heap allocations during runtime tick loops.

use eidolon_core::{evaluate_perception, BehaviorTree, SensoryProfile};

use crate::threat::ThreatTable;

/// State machine states for autonomous NPC entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NpcState {
    /// Idling at or near spawn origin.
    Idle,
    /// Patrolling between designated waypoints.
    Patrolling,
    /// Actively pursuing a targeted combatant.
    Chasing,
    /// Within attack range, executing combat abilities.
    Attacking,
    /// Leashed or reset: invulnerable, clearing threat, pathing back to spawn origin.
    Evading,
}

/// Simulation LOD tier determining tick frequency for each NPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NpcLodTier {
    /// In combat or within immediate player observer range (<15m): 20 Hz simulation.
    ActiveCombat,
    /// Within medium player observer range (15m - 50m): 2 Hz simulation.
    MidTier,
    /// Wilderness with zero players within 50m: 0 Hz dormant hibernation.
    Dormant,
}

/// A simulated autonomous NPC entity.
#[derive(Debug, Clone)]
pub struct NpcEntity {
    /// Unique entity ID.
    pub entity_id: u64,
    /// NPC archetype / template ID.
    pub type_id: u32,
    /// Canonical spawn position and leashing anchor.
    pub spawn_origin: (f32, f32, f32),
    /// Current world position.
    pub position: (f32, f32, f32),
    /// Current yaw heading in degrees.
    pub yaw_degrees: f32,
    /// Current health pool.
    pub health: u32,
    /// Maximum health pool.
    pub max_health: u32,
    /// Maximum leash radius in meters before triggering Evade reset.
    pub leash_radius: f32,
    /// Sensory perception profile (sight cone, hearing, stealth).
    pub sensory: SensoryProfile,
    /// Authoritative threat table.
    pub threat_table: ThreatTable,
    /// Decision behavior tree.
    pub behavior_tree: BehaviorTree,
    /// Active attack target entity ID.
    pub current_target: Option<u64>,
    /// Active state machine state.
    pub state: NpcState,
    /// Active simulation Level of Detail.
    pub lod_tier: NpcLodTier,
    /// Last tick index when this NPC was simulated.
    pub last_tick: u64,
}

impl NpcEntity {
    /// Creates a new NPC entity at its spawn origin.
    pub fn new(
        entity_id: u64,
        type_id: u32,
        spawn_origin: (f32, f32, f32),
        max_health: u32,
        leash_radius: f32,
        sensory: SensoryProfile,
    ) -> Self {
        Self {
            entity_id,
            type_id,
            spawn_origin,
            position: spawn_origin,
            yaw_degrees: 0.0,
            health: max_health,
            max_health,
            leash_radius: leash_radius.max(10.0),
            sensory,
            threat_table: ThreatTable::new(),
            behavior_tree: BehaviorTree::new(),
            current_target: None,
            state: NpcState::Idle,
            lod_tier: NpcLodTier::Dormant,
            last_tick: 0,
        }
    }

    /// Checks whether the entity has exceeded its maximum leash radius from spawn origin.
    pub fn is_beyond_leash(&self) -> bool {
        let dx = self.position.0 - self.spawn_origin.0;
        let dz = self.position.2 - self.spawn_origin.2;
        dx * dx + dz * dz > self.leash_radius * self.leash_radius
    }
}

/// Central ecosystem coordinator managing NPC lifecycles, aggro, and simulation LOD.
pub struct NpcEcosystemManager {
    npcs: Vec<NpcEntity>,
}

impl Default for NpcEcosystemManager {
    fn default() -> Self {
        Self {
            npcs: Vec::with_capacity(256),
        }
    }
}

impl NpcEcosystemManager {
    /// Creates a new empty NPC ecosystem manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawns a new autonomous NPC into the world.
    pub fn spawn_npc(
        &mut self,
        entity_id: u64,
        type_id: u32,
        spawn_origin: (f32, f32, f32),
        max_health: u32,
        leash_radius: f32,
        sensory: SensoryProfile,
    ) {
        let npc = NpcEntity::new(
            entity_id,
            type_id,
            spawn_origin,
            max_health,
            leash_radius,
            sensory,
        );
        self.npcs.push(npc);
    }

    /// Adds an existing NPC instance to the ecosystem manager.
    pub fn add_npc(&mut self, npc: NpcEntity) {
        self.npcs.push(npc);
    }

    /// Returns a slice of all currently managed NPCs.
    pub fn npcs(&self) -> &[NpcEntity] {
        &self.npcs
    }

    /// Returns the total count of managed NPCs.
    pub fn len(&self) -> usize {
        self.npcs.len()
    }

    /// Checks if the manager has zero NPCs.
    pub fn is_empty(&self) -> bool {
        self.npcs.is_empty()
    }

    /// Retrieves an immutable reference to an NPC by entity ID.
    pub fn get_npc(&self, entity_id: u64) -> Option<&NpcEntity> {
        self.npcs.iter().find(|n| n.entity_id == entity_id)
    }

    /// Retrieves a mutable reference to an NPC by entity ID.
    pub fn get_npc_mut(&mut self, entity_id: u64) -> Option<&mut NpcEntity> {
        self.npcs.iter_mut().find(|n| n.entity_id == entity_id)
    }

    /// Updates simulation LOD tiers across all NPCs based on proximity to active players.
    pub fn update_lod_tiers(&mut self, player_positions: &[(f32, f32, f32)]) {
        for npc in &mut self.npcs {
            // If in active combat, always run at highest tier
            if npc.state == NpcState::Chasing || npc.state == NpcState::Attacking {
                npc.lod_tier = NpcLodTier::ActiveCombat;
                continue;
            }

            let mut min_dist_sq = f32::MAX;
            for &p_pos in player_positions {
                let dx = npc.position.0 - p_pos.0;
                let dz = npc.position.2 - p_pos.2;
                let dist_sq = dx * dx + dz * dz;
                if dist_sq < min_dist_sq {
                    min_dist_sq = dist_sq;
                }
            }

            if min_dist_sq <= 15.0 * 15.0 {
                npc.lod_tier = NpcLodTier::ActiveCombat;
            } else if min_dist_sq <= 50.0 * 50.0 {
                npc.lod_tier = NpcLodTier::MidTier;
            } else {
                npc.lod_tier = NpcLodTier::Dormant;
            }
        }
    }

    /// Inflicts damage on an NPC and credits threat to the attacker.
    pub fn apply_damage(
        &mut self,
        npc_entity_id: u64,
        attacker_id: u64,
        damage_amount: u32,
        current_tick: u64,
    ) {
        if let Some(npc) = self.get_npc_mut(npc_entity_id) {
            // Evading mobs are immune to damage
            if npc.state == NpcState::Evading {
                return;
            }

            npc.health = npc.health.saturating_sub(damage_amount);
            npc.threat_table
                .add_threat(attacker_id, damage_amount, true, current_tick);
            npc.lod_tier = NpcLodTier::ActiveCombat;

            // Transition from Idle/Patrolling to Chasing upon taking damage
            if npc.state == NpcState::Idle || npc.state == NpcState::Patrolling {
                npc.state = NpcState::Chasing;
                npc.current_target = Some(attacker_id);
            }
        }
    }

    /// Applies a tank taunt lock on an NPC for a set duration.
    pub fn apply_taunt(
        &mut self,
        npc_entity_id: u64,
        tank_id: u64,
        duration_ticks: u64,
        current_tick: u64,
    ) {
        if let Some(npc) = self.get_npc_mut(npc_entity_id) {
            if npc.state != NpcState::Evading {
                npc.lod_tier = NpcLodTier::ActiveCombat;
                npc.threat_table
                    .apply_taunt(tank_id, duration_ticks, current_tick);
                npc.current_target = Some(tank_id);
            }
        }
    }

    /// Main simulation tick updating active NPCs according to their simulation LOD tiers.
    pub fn tick(
        &mut self,
        current_tick: u64,
        _delta_time: f32,
        player_positions: &[(u64, (f32, f32, f32), u32)], // (player_id, pos, stealth)
    ) {
        for npc in &mut self.npcs {
            // Simulation LOD filter
            match npc.lod_tier {
                NpcLodTier::Dormant => continue, // Sleep: 0 CPU cost
                NpcLodTier::MidTier => {
                    // 2 Hz tick: update once every 10 ticks (at 20 Hz server clock)
                    if !current_tick.is_multiple_of(10) {
                        continue;
                    }
                }
                NpcLodTier::ActiveCombat => {} // Full 20 Hz tick
            }

            npc.last_tick = current_tick;

            // 1. Check leashing boundary
            if npc.is_beyond_leash() && npc.state != NpcState::Evading {
                npc.state = NpcState::Evading;
                npc.threat_table.clear();
                npc.current_target = None;
            }

            // 2. State machine execution
            match npc.state {
                NpcState::Evading => {
                    // Walk back towards spawn origin
                    let dx = npc.spawn_origin.0 - npc.position.0;
                    let dz = npc.spawn_origin.2 - npc.position.2;
                    let dist_sq = dx * dx + dz * dz;

                    if dist_sq < 1.0 {
                        // Reached spawn origin: restore full health, return to Idle
                        npc.position = npc.spawn_origin;
                        npc.health = npc.max_health;
                        npc.state = NpcState::Idle;
                    } else {
                        let dist = dist_sq.sqrt();
                        let move_step = 0.5f32.min(dist);
                        npc.position.0 += (dx / dist) * move_step;
                        npc.position.2 += (dz / dist) * move_step;
                    }
                }

                NpcState::Idle | NpcState::Patrolling => {
                    // Check sensory perception against players
                    for &(player_id, p_pos, p_stealth) in player_positions {
                        let perception = evaluate_perception(
                            npc.position,
                            npc.yaw_degrees,
                            p_pos,
                            p_stealth,
                            &npc.sensory,
                        );

                        if perception.perceived {
                            // Aggro detected!
                            npc.threat_table
                                .add_threat(player_id, 100, false, current_tick);
                            npc.current_target = Some(player_id);
                            npc.state = NpcState::Chasing;
                            break;
                        }
                    }
                }

                NpcState::Chasing => {
                    // Target selection with hysteresis
                    npc.current_target =
                        npc.threat_table
                            .select_target(npc.current_target, false, current_tick);

                    if npc.current_target.is_none() {
                        npc.state = NpcState::Evading;
                    } else if let Some(target_id) = npc.current_target {
                        // Find target position
                        if let Some(&(_, p_pos, _)) =
                            player_positions.iter().find(|(id, _, _)| *id == target_id)
                        {
                            let dx = p_pos.0 - npc.position.0;
                            let dz = p_pos.2 - npc.position.2;
                            let dist = (dx * dx + dz * dz).sqrt();

                            if dist <= 2.5 {
                                // Within attack range
                                npc.state = NpcState::Attacking;
                            } else {
                                // Move towards target
                                let move_step = 0.3f32.min(dist);
                                npc.position.0 += (dx / dist) * move_step;
                                npc.position.2 += (dz / dist) * move_step;
                            }
                        } else {
                            // Target lost/logged out
                            npc.threat_table.remove_entity(target_id);
                        }
                    }
                }

                NpcState::Attacking => {
                    // Re-verify target in melee range
                    npc.current_target =
                        npc.threat_table
                            .select_target(npc.current_target, true, current_tick);

                    if let Some(target_id) = npc.current_target {
                        if let Some(&(_, p_pos, _)) =
                            player_positions.iter().find(|(id, _, _)| *id == target_id)
                        {
                            let dx = p_pos.0 - npc.position.0;
                            let dz = p_pos.2 - npc.position.2;
                            let dist = (dx * dx + dz * dz).sqrt();
                            if dist > 3.0 {
                                npc.state = NpcState::Chasing;
                            }
                        } else {
                            npc.threat_table.remove_entity(target_id);
                            npc.state = NpcState::Chasing;
                        }
                    } else {
                        npc.state = NpcState::Evading;
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
    fn test_npc_spawn_and_initial_dormant_lod() {
        let mut manager = NpcEcosystemManager::new();
        let sensory = SensoryProfile::default();
        manager.spawn_npc(501, 1, (100.0, 0.0, 100.0), 1000, 50.0, sensory);

        let npc = manager.get_npc(501).unwrap();
        assert_eq!(npc.health, 1000);
        assert_eq!(npc.state, NpcState::Idle);
        assert_eq!(npc.lod_tier, NpcLodTier::Dormant);
    }

    #[test]
    fn test_update_lod_tiers_by_distance() {
        let mut manager = NpcEcosystemManager::new();
        let sensory = SensoryProfile::default();
        manager.spawn_npc(501, 1, (0.0, 0.0, 0.0), 1000, 50.0, sensory); // at (0, 0, 0)

        // Player at 10m -> ActiveCombat (<15m)
        manager.update_lod_tiers(&[(10.0, 0.0, 0.0)]);
        assert_eq!(
            manager.get_npc(501).unwrap().lod_tier,
            NpcLodTier::ActiveCombat
        );

        // Player at 30m -> MidTier (15m - 50m)
        manager.update_lod_tiers(&[(30.0, 0.0, 0.0)]);
        assert_eq!(manager.get_npc(501).unwrap().lod_tier, NpcLodTier::MidTier);

        // Player at 100m -> Dormant (>50m)
        manager.update_lod_tiers(&[(100.0, 0.0, 0.0)]);
        assert_eq!(manager.get_npc(501).unwrap().lod_tier, NpcLodTier::Dormant);
    }

    #[test]
    fn test_leashing_evade_and_health_restoration() {
        let mut manager = NpcEcosystemManager::new();
        let sensory = SensoryProfile::default();
        manager.spawn_npc(501, 1, (0.0, 0.0, 0.0), 1000, 20.0, sensory); // leash = 20m

        // Move NPC to 25m (beyond 20m leash) and apply damage
        manager.apply_damage(501, 101, 400, 1);
        let npc_mut = manager.get_npc_mut(501).unwrap();
        npc_mut.position = (25.0, 0.0, 0.0);

        assert_eq!(npc_mut.health, 600);

        // Tick simulation
        manager.tick(1, 0.05, &[(101, (25.0, 0.0, 0.0), 0)]);

        // Must transition to Evading and clear threat
        let npc_after = manager.get_npc(501).unwrap();
        assert_eq!(npc_after.state, NpcState::Evading);
        assert!(npc_after.threat_table.is_empty());
    }
}
