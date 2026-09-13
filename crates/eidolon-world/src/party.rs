//! Group party management, invite workflows, and party vitals replication.
//!
//! Provides authoritative party topologies supporting up to 8 members per group
//! with leader authority controls and real-time vital state synchronization.

use std::collections::HashMap;

use eidolon_core::fixed::Vec3Fix;

use crate::error::WorldError;

/// Maximum allowable players in a single party group.
pub const MAX_GROUP_MEMBERS: usize = 8;

/// Snapshot of an individual party member's combat vitals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartyMember {
    /// Persistent account identifier.
    pub account_id: u64,
    /// Authoritative entity identifier in active world simulation.
    pub entity_id: u32,
    /// Character display name.
    pub name: String,
    /// Current health points.
    pub health: u32,
    /// Maximum health points.
    pub max_health: u32,
    /// Current mana / resource points.
    pub mana: u32,
    /// Maximum mana / resource points.
    pub max_mana: u32,
    /// Current world spatial coordinates.
    pub position: Vec3Fix,
}

/// Authoritative party group entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Party {
    /// Unique party group identifier.
    pub party_id: u64,
    /// Account identifier of the party leader.
    pub leader_account_id: u64,
    /// List of current active party members (up to `MAX_GROUP_MEMBERS`).
    pub members: Vec<PartyMember>,
}

impl Party {
    /// Creates a new party with the given member as the initial leader.
    pub fn new(party_id: u64, leader: PartyMember) -> Self {
        let leader_id = leader.account_id;
        Self {
            party_id,
            leader_account_id: leader_id,
            members: vec![leader],
        }
    }

    /// Checks if an account is an active member of this party.
    pub fn is_member(&self, account_id: u64) -> bool {
        self.members.iter().any(|m| m.account_id == account_id)
    }

    /// Checks if an account is the designated leader.
    #[inline]
    pub fn is_leader(&self, account_id: u64) -> bool {
        self.leader_account_id == account_id
    }

    /// Adds a new member to the party, verifying party capacity.
    pub fn add_member(&mut self, member: PartyMember) -> Result<(), WorldError> {
        if self.members.len() >= MAX_GROUP_MEMBERS {
            return Err(WorldError::TransactionAborted("Party is full"));
        }
        if self.is_member(member.account_id) {
            return Err(WorldError::TransactionAborted("Player already in party"));
        }
        self.members.push(member);
        Ok(())
    }

    /// Removes a member from the party.
    ///
    /// If the leader leaves, automatically promotes the next oldest member.
    /// Returns `true` if the party became completely empty.
    pub fn remove_member(&mut self, account_id: u64) -> Result<bool, WorldError> {
        let pos = self
            .members
            .iter()
            .position(|m| m.account_id == account_id)
            .ok_or(WorldError::TransactionAborted("Player not in party"))?;

        self.members.remove(pos);

        if self.members.is_empty() {
            return Ok(true);
        }

        // If leader left, promote the next member
        if self.leader_account_id == account_id {
            self.leader_account_id = self.members[0].account_id;
        }

        Ok(false)
    }

    /// Promotes an existing member to party leader.
    pub fn promote_leader(&mut self, new_leader_id: u64) -> Result<(), WorldError> {
        if !self.is_member(new_leader_id) {
            return Err(WorldError::TransactionAborted("Player not in party"));
        }
        self.leader_account_id = new_leader_id;
        Ok(())
    }

    /// Updates the dynamic vitals for an existing member.
    pub fn update_member_vitals(
        &mut self,
        account_id: u64,
        health: u32,
        mana: u32,
        position: Vec3Fix,
    ) {
        if let Some(m) = self.members.iter_mut().find(|m| m.account_id == account_id) {
            m.health = health;
            m.mana = mana;
            m.position = position;
        }
    }
}

/// Global party manager managing group lifecycles and pending invitations.
#[derive(Debug, Default)]
pub struct PartyManager {
    next_party_id: u64,
    parties: HashMap<u64, Party>,
    account_to_party: HashMap<u64, u64>,
    pending_invites: HashMap<u64, u64>, // target_account_id -> party_id
}

impl PartyManager {
    /// Constructs a new empty party manager.
    pub fn new() -> Self {
        Self {
            next_party_id: 1,
            parties: HashMap::new(),
            account_to_party: HashMap::new(),
            pending_invites: HashMap::new(),
        }
    }

    /// Creates a new party with the given member as leader.
    pub fn create_party(&mut self, leader: PartyMember) -> Result<u64, WorldError> {
        let leader_id = leader.account_id;
        if self.account_to_party.contains_key(&leader_id) {
            return Err(WorldError::TransactionAborted("Player already in a party"));
        }

        let pid = self.next_party_id;
        self.next_party_id += 1;

        let party = Party::new(pid, leader);
        self.parties.insert(pid, party);
        self.account_to_party.insert(leader_id, pid);
        Ok(pid)
    }

    /// Issues an invitation from a party leader to a target account.
    pub fn invite_player(
        &mut self,
        party_id: u64,
        caller_account: u64,
        target_account: u64,
    ) -> Result<(), WorldError> {
        let party = self
            .parties
            .get(&party_id)
            .ok_or(WorldError::TransactionAborted("Party does not exist"))?;

        if !party.is_leader(caller_account) {
            return Err(WorldError::TransactionAborted(
                "Only party leader can invite",
            ));
        }

        if party.members.len() >= MAX_GROUP_MEMBERS {
            return Err(WorldError::TransactionAborted("Party is full"));
        }

        if self.account_to_party.contains_key(&target_account) {
            return Err(WorldError::TransactionAborted(
                "Target player already in a party",
            ));
        }

        self.pending_invites.insert(target_account, party_id);
        Ok(())
    }

    /// Accepts a pending invitation, adding the member to the party.
    pub fn accept_invite(
        &mut self,
        target_account: u64,
        member: PartyMember,
    ) -> Result<u64, WorldError> {
        let party_id =
            self.pending_invites
                .remove(&target_account)
                .ok_or(WorldError::TransactionAborted(
                    "No pending invitation found",
                ))?;

        let party = self
            .parties
            .get_mut(&party_id)
            .ok_or(WorldError::TransactionAborted("Party no longer exists"))?;

        party.add_member(member)?;
        self.account_to_party.insert(target_account, party_id);
        Ok(party_id)
    }

    /// Declines or cancels a pending invitation.
    pub fn decline_invite(&mut self, target_account: u64) {
        self.pending_invites.remove(&target_account);
    }

    /// Leaves the current party. Returns the party ID if left, or None if not in a party.
    pub fn leave_party(&mut self, account_id: u64) -> Result<Option<u64>, WorldError> {
        let party_id = match self.account_to_party.remove(&account_id) {
            Some(pid) => pid,
            None => return Ok(None),
        };

        if let Some(party) = self.parties.get_mut(&party_id) {
            let empty = party.remove_member(account_id)?;
            if empty {
                self.parties.remove(&party_id);
            }
        }

        Ok(Some(party_id))
    }

    /// Retrieves an immutable reference to a party by ID.
    pub fn get_party(&self, party_id: u64) -> Option<&Party> {
        self.parties.get(&party_id)
    }

    /// Retrieves a mutable reference to a party by ID.
    pub fn get_party_mut(&mut self, party_id: u64) -> Option<&mut Party> {
        self.parties.get_mut(&party_id)
    }

    /// Retrieves an immutable reference to a party by member account ID.
    pub fn get_party_by_account(&self, account_id: u64) -> Option<&Party> {
        let pid = self.account_to_party.get(&account_id)?;
        self.parties.get(pid)
    }

    /// Retrieves a mutable reference to a party by member account ID.
    pub fn get_party_by_account_mut(&mut self, account_id: u64) -> Option<&mut Party> {
        let pid = *self.account_to_party.get(&account_id)?;
        self.parties.get_mut(&pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_member(id: u64, name: &'static str) -> PartyMember {
        PartyMember {
            account_id: id,
            entity_id: id as u32,
            name: name.to_string(),
            health: 100,
            max_health: 100,
            mana: 50,
            max_mana: 50,
            position: Vec3Fix::ZERO,
        }
    }

    #[test]
    fn test_party_lifecycle_and_invitations() {
        let mut manager = PartyManager::new();

        let leader = make_member(1001, "Sir Galahad");
        let pid = manager.create_party(leader).expect("Create party");

        assert_eq!(manager.get_party(pid).unwrap().members.len(), 1);
        assert!(manager.get_party(pid).unwrap().is_leader(1001));

        // Invite player 1002
        manager.invite_player(pid, 1001, 1002).expect("Invite 1002");

        // Non-leader cannot invite
        let err = manager.invite_player(pid, 1002, 1003);
        assert!(err.is_err());

        // Player 1002 accepts
        let member_2 = make_member(1002, "Lady Guinevere");
        let accepted_pid = manager
            .accept_invite(1002, member_2)
            .expect("Accept invite");
        assert_eq!(accepted_pid, pid);

        assert_eq!(manager.get_party(pid).unwrap().members.len(), 2);
        assert!(manager.get_party(pid).unwrap().is_member(1002));

        // Update vitals
        if let Some(party) = manager.get_party_by_account_mut(1002) {
            party.update_member_vitals(1002, 85, 40, Vec3Fix::from_f64(10.0, 0.0, 10.0));
        }

        let party = manager.get_party(pid).unwrap();
        let m2 = party.members.iter().find(|m| m.account_id == 1002).unwrap();
        assert_eq!(m2.health, 85);

        // Leader leaves: next member automatically promoted to leader
        manager.leave_party(1001).expect("Leader leaves");
        let party = manager.get_party(pid).unwrap();
        assert_eq!(party.members.len(), 1);
        assert_eq!(party.leader_account_id, 1002);

        // Last member leaves: party is disbanded
        manager.leave_party(1002).expect("Last member leaves");
        assert!(manager.get_party(pid).is_none());
    }
}
