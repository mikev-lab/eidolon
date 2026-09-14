//! Social Institutions, Guilds, Alliances & Dynamic Faction Standing.
//!
//! Provides 8-tier guild rank hierarchies with customizable permission bitmasks,
//! inter-guild alliance treaties, and dynamic diplomatic standings (-10,000 to +10,000).

use std::collections::HashMap;

/// Maximum allowable guild members.
pub const MAX_GUILD_MEMBERS: usize = 500;
/// Maximum name length in bytes.
pub const MAX_NAME_LEN: usize = 32;
/// Maximum guild tag length in bytes.
pub const MAX_TAG_LEN: usize = 6;

/// Permission to invite new recruits to the guild.
pub const GUILD_PERM_INVITE: u32 = 1 << 0;
/// Permission to kick lower-ranked members from the guild.
pub const GUILD_PERM_KICK: u32 = 1 << 1;
/// Permission to promote lower-ranked members.
pub const GUILD_PERM_PROMOTE: u32 = 1 << 2;
/// Permission to demote lower-ranked members.
pub const GUILD_PERM_DEMOTE: u32 = 1 << 3;
/// Permission to deposit items and currency into the guild vault.
pub const GUILD_PERM_VAULT_DEPOSIT: u32 = 1 << 4;
/// Permission to withdraw items and currency from the guild vault.
pub const GUILD_PERM_VAULT_WITHDRAW: u32 = 1 << 5;
/// Permission to claim world territory plots in the guild's name.
pub const GUILD_PERM_CLAIM_TERRITORY: u32 = 1 << 6;
/// Permission to adjust diplomatic relations with other guilds.
pub const GUILD_PERM_MANAGE_DIPLOMACY: u32 = 1 << 7;
/// Permission to configure rank permissions.
pub const GUILD_PERM_EDIT_RANKS: u32 = 1 << 8;

/// Errors arising from social graph operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocialError {
    /// Guild already exists with this ID or tag.
    GuildAlreadyExists,
    /// Guild not found.
    GuildNotFound,
    /// Player is already in a guild.
    PlayerAlreadyInGuild,
    /// Player not found in guild roster.
    PlayerNotInGuild,
    /// Guild member limit reached.
    GuildFull,
    /// Insufficient rank permissions for action.
    PermissionDenied,
    /// Cannot kick or demote the guild master.
    CannotModifyGuildMaster,
    /// Target rank index is out of bounds (0-7).
    InvalidRank,
    /// Alliance not found.
    AllianceNotFound,
    /// Guild is already in an alliance.
    GuildAlreadyInAlliance,
}

impl core::fmt::Display for SocialError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::GuildAlreadyExists => write!(f, "Guild already exists"),
            Self::GuildNotFound => write!(f, "Guild not found"),
            Self::PlayerAlreadyInGuild => write!(f, "Player already in a guild"),
            Self::PlayerNotInGuild => write!(f, "Player not in guild"),
            Self::GuildFull => write!(f, "Guild member limit reached"),
            Self::PermissionDenied => write!(f, "Insufficient guild permissions"),
            Self::CannotModifyGuildMaster => write!(f, "Cannot modify guild master"),
            Self::InvalidRank => write!(f, "Invalid rank index (must be 0-7)"),
            Self::AllianceNotFound => write!(f, "Alliance not found"),
            Self::GuildAlreadyInAlliance => write!(f, "Guild already in an alliance"),
        }
    }
}

impl std::error::Error for SocialError {}

/// Diplomatic standing status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiplomaticStatus {
    /// Score <= -6,000: Open war / PvP on sight without penalty.
    AtWar,
    /// Score between -5,999 and -2,000: Hostile.
    Hostile,
    /// Score between -1,999 and +1,999: Neutral.
    Neutral,
    /// Score between +2,000 and +5,999: Friendly.
    Friendly,
    /// Score >= +6,000: Formal Alliance.
    Allied,
}

/// A member of a guild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildMember {
    /// Member account ID.
    pub account_id: u64,
    /// Primary character ID.
    pub character_id: u64,
    /// Rank tier: 0 = Recruit, up to 7 = Guild Master.
    pub rank: u8,
    /// Tick timestamp when player joined.
    pub joined_tick: u64,
}

/// Guild institution managing members, ranks, and permissions.
#[derive(Debug, Clone)]
pub struct Guild {
    /// Unique guild ID.
    pub guild_id: u64,
    /// Guild name string.
    pub name: String,
    /// Short guild tag.
    pub tag: String,
    /// Account ID of the guild master.
    pub master_account_id: u64,
    /// Roster of active members keyed by account ID.
    pub members: HashMap<u64, GuildMember>,
    /// Permission bitmasks for each of the 8 ranks (0 = Recruit, 7 = Master).
    pub rank_permissions: [u32; 8],
    /// Parent alliance ID if part of an alliance.
    pub alliance_id: Option<u64>,
}

impl Guild {
    /// Creates a new guild with default permissions.
    pub fn new(
        guild_id: u64,
        name: impl Into<String>,
        tag: impl Into<String>,
        master_account_id: u64,
        master_character_id: u64,
        current_tick: u64,
    ) -> Self {
        let mut members = HashMap::new();
        members.insert(
            master_account_id,
            GuildMember {
                account_id: master_account_id,
                character_id: master_character_id,
                rank: 7, // Guild Master rank
                joined_tick: current_tick,
            },
        );

        // Default permission hierarchy across the 8 ranks:
        // Rank 0 (Recruit): None
        // Rank 1 (Member): Deposit
        // Rank 2 (Veteran): Deposit, Withdraw
        // Rank 3 (Officer): Invite, Kick, Deposit, Withdraw
        // Rank 4 (Lieutenant): Invite, Kick, Promote, Demote, Deposit, Withdraw
        // Rank 5 (General): All above + Claim Territory
        // Rank 6 (Co-Leader): All above + Manage Diplomacy
        // Rank 7 (Master): All permissions (u32::MAX)
        let mut rank_permissions = [0u32; 8];
        rank_permissions[0] = 0;
        rank_permissions[1] = GUILD_PERM_VAULT_DEPOSIT;
        rank_permissions[2] = GUILD_PERM_VAULT_DEPOSIT | GUILD_PERM_VAULT_WITHDRAW;
        rank_permissions[3] = rank_permissions[2] | GUILD_PERM_INVITE | GUILD_PERM_KICK;
        rank_permissions[4] = rank_permissions[3] | GUILD_PERM_PROMOTE | GUILD_PERM_DEMOTE;
        rank_permissions[5] = rank_permissions[4] | GUILD_PERM_CLAIM_TERRITORY;
        rank_permissions[6] = rank_permissions[5] | GUILD_PERM_MANAGE_DIPLOMACY;
        rank_permissions[7] = u32::MAX; // Master has all permissions

        Self {
            guild_id,
            name: name.into(),
            tag: tag.into(),
            master_account_id,
            members,
            rank_permissions,
            alliance_id: None,
        }
    }

    /// Checks if a member possesses a specific permission bit.
    pub fn has_permission(&self, account_id: u64, permission: u32) -> bool {
        if account_id == self.master_account_id {
            return true;
        }

        if let Some(member) = self.members.get(&account_id) {
            let perms = self.rank_permissions[member.rank as usize];
            (perms & permission) == permission
        } else {
            false
        }
    }

    /// Adds a new recruit to the guild.
    pub fn add_member(
        &mut self,
        account_id: u64,
        character_id: u64,
        current_tick: u64,
    ) -> Result<(), SocialError> {
        if self.members.len() >= MAX_GUILD_MEMBERS {
            return Err(SocialError::GuildFull);
        }

        if self.members.contains_key(&account_id) {
            return Err(SocialError::PlayerAlreadyInGuild);
        }

        self.members.insert(
            account_id,
            GuildMember {
                account_id,
                character_id,
                rank: 0, // Recruits start at rank 0
                joined_tick: current_tick,
            },
        );

        Ok(())
    }

    /// Removes a member from the guild.
    pub fn remove_member(&mut self, account_id: u64) -> Result<GuildMember, SocialError> {
        if account_id == self.master_account_id {
            return Err(SocialError::CannotModifyGuildMaster);
        }

        self.members
            .remove(&account_id)
            .ok_or(SocialError::PlayerNotInGuild)
    }

    /// Promotes or demotes a member to a specified rank.
    pub fn set_rank(
        &mut self,
        actor_account_id: u64,
        target_account_id: u64,
        new_rank: u8,
    ) -> Result<(), SocialError> {
        if new_rank > 7 {
            return Err(SocialError::InvalidRank);
        }

        if target_account_id == self.master_account_id {
            return Err(SocialError::CannotModifyGuildMaster);
        }

        let actor_rank = self
            .members
            .get(&actor_account_id)
            .map(|m| m.rank)
            .ok_or(SocialError::PlayerNotInGuild)?;

        // Actors can only grant ranks strictly below their own rank
        if actor_rank <= new_rank {
            return Err(SocialError::PermissionDenied);
        }

        let member = self
            .members
            .get_mut(&target_account_id)
            .ok_or(SocialError::PlayerNotInGuild)?;

        member.rank = new_rank;
        Ok(())
    }
}

/// Multi-guild alliance coalition.
#[derive(Debug, Clone)]
pub struct Alliance {
    /// Unique alliance ID.
    pub alliance_id: u64,
    /// Alliance name.
    pub name: String,
    /// Founding leader guild ID.
    pub leader_guild_id: u64,
    /// Member guild IDs.
    pub member_guild_ids: Vec<u64>,
}

impl Alliance {
    /// Creates a new alliance.
    pub fn new(alliance_id: u64, name: impl Into<String>, leader_guild_id: u64) -> Self {
        Self {
            alliance_id,
            name: name.into(),
            leader_guild_id,
            member_guild_ids: vec![leader_guild_id],
        }
    }

    /// Checks if a guild is in this alliance.
    pub fn contains_guild(&self, guild_id: u64) -> bool {
        self.member_guild_ids.contains(&guild_id)
    }

    /// Adds a guild to the alliance.
    pub fn add_guild(&mut self, guild_id: u64) -> Result<(), SocialError> {
        if self.contains_guild(guild_id) {
            return Err(SocialError::GuildAlreadyInAlliance);
        }
        self.member_guild_ids.push(guild_id);
        Ok(())
    }

    /// Removes a guild from the alliance.
    pub fn remove_guild(&mut self, guild_id: u64) -> Result<(), SocialError> {
        if guild_id == self.leader_guild_id {
            return Err(SocialError::CannotModifyGuildMaster);
        }

        if let Some(pos) = self.member_guild_ids.iter().position(|&g| g == guild_id) {
            self.member_guild_ids.remove(pos);
            Ok(())
        } else {
            Err(SocialError::GuildNotFound)
        }
    }
}

/// Global social institution and diplomacy manager.
#[derive(Debug, Default)]
pub struct SocialManager {
    guilds: HashMap<u64, Guild>,
    alliances: HashMap<u64, Alliance>,
    // Diplomatic standing scores between pairs of entities/guilds: (src, dst) -> score (-10,000 to +10,000)
    standings: HashMap<(u64, u64), i32>,
}

impl SocialManager {
    /// Creates a new social manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new guild.
    pub fn create_guild(&mut self, guild: Guild) -> Result<(), SocialError> {
        if self.guilds.contains_key(&guild.guild_id) {
            return Err(SocialError::GuildAlreadyExists);
        }
        self.guilds.insert(guild.guild_id, guild);
        Ok(())
    }

    /// Retrieves an immutable reference to a guild.
    pub fn get_guild(&self, guild_id: u64) -> Option<&Guild> {
        self.guilds.get(&guild_id)
    }

    /// Retrieves a mutable reference to a guild.
    pub fn get_guild_mut(&mut self, guild_id: u64) -> Option<&mut Guild> {
        self.guilds.get_mut(&guild_id)
    }

    /// Registers a new alliance.
    pub fn create_alliance(&mut self, alliance: Alliance) -> Result<(), SocialError> {
        if self.alliances.contains_key(&alliance.alliance_id) {
            return Err(SocialError::GuildAlreadyExists);
        }
        self.alliances.insert(alliance.alliance_id, alliance);
        Ok(())
    }

    /// Sets or adjusts diplomatic standing between two entities or guilds.
    pub fn set_standing(&mut self, source_id: u64, target_id: u64, score: i32) {
        let clamped = score.clamp(-10_000, 10_000);
        self.standings.insert((source_id, target_id), clamped);
    }

    /// Retrieves diplomatic standing score between two entities (-10,000 to +10,000).
    pub fn get_standing_score(&self, source_id: u64, target_id: u64) -> i32 {
        if source_id == target_id {
            return 10_000; // Self is always maximum allied
        }
        self.standings
            .get(&(source_id, target_id))
            .copied()
            .unwrap_or(0) // Default neutral
    }

    /// Evaluates the qualitative diplomatic relationship between two entities.
    pub fn get_diplomatic_status(&self, source_id: u64, target_id: u64) -> DiplomaticStatus {
        let score = self.get_standing_score(source_id, target_id);
        if score <= -6000 {
            DiplomaticStatus::AtWar
        } else if score <= -2000 {
            DiplomaticStatus::Hostile
        } else if score < 2000 {
            DiplomaticStatus::Neutral
        } else if score < 6000 {
            DiplomaticStatus::Friendly
        } else {
            DiplomaticStatus::Allied
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guild_creation_and_permission_hierarchy() {
        let mut guild = Guild::new(1, "Order of the Phoenix", "PHX", 101, 1001, 1);
        assert_eq!(guild.master_account_id, 101);
        assert!(guild.has_permission(101, GUILD_PERM_INVITE));
        assert!(guild.has_permission(101, GUILD_PERM_CLAIM_TERRITORY));

        // Recruit player 202
        guild.add_member(202, 2002, 2).unwrap();
        // Recruit has rank 0: No permissions
        assert!(!guild.has_permission(202, GUILD_PERM_VAULT_WITHDRAW));
        assert!(!guild.has_permission(202, GUILD_PERM_INVITE));

        // Promote recruit to Veteran (Rank 2)
        guild.set_rank(101, 202, 2).unwrap();
        assert!(guild.has_permission(202, GUILD_PERM_VAULT_WITHDRAW));
        assert!(!guild.has_permission(202, GUILD_PERM_INVITE));

        // Promote to Officer (Rank 3)
        guild.set_rank(101, 202, 3).unwrap();
        assert!(guild.has_permission(202, GUILD_PERM_INVITE));
        assert!(!guild.has_permission(202, GUILD_PERM_CLAIM_TERRITORY));
    }

    #[test]
    fn test_diplomatic_standing_transitions() {
        let mut social = SocialManager::new();

        // Default neutral
        assert_eq!(
            social.get_diplomatic_status(1, 2),
            DiplomaticStatus::Neutral
        );

        // Declare war (-8,000)
        social.set_standing(1, 2, -8000);
        assert_eq!(social.get_diplomatic_status(1, 2), DiplomaticStatus::AtWar);

        // Form peace treaty (+3,000)
        social.set_standing(1, 2, 3000);
        assert_eq!(
            social.get_diplomatic_status(1, 2),
            DiplomaticStatus::Friendly
        );

        // Formal alliance (+8,000)
        social.set_standing(1, 2, 8000);
        assert_eq!(social.get_diplomatic_status(1, 2), DiplomaticStatus::Allied);
    }
}
