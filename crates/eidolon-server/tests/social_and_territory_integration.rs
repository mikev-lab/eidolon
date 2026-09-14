//! Exhaustive integration test suite for Phase 23: Social Institutions, Territory Sovereignty & Dynamic Permissions.
//!
//! Validates:
//! 1. 8-tier guild rank hierarchy and dynamic permission bitmasks.
//! 2. Multi-guild alliance treaties and coalition membership.
//! 3. Spatial territory plot bounding box lookups and upkeep lifecycles.
//! 4. Access Control List (ACL) permission matrices across Guild, Alliance, and Public actors.
//! 5. Diplomatic faction standing shifts (-10,000 to +10,000) and war declarations.

use eidolon_world::{
    Alliance, DiplomaticStatus, Guild, SocialError, SocialManager, TerritoryManager, TerritoryPlot,
    GUILD_PERM_CLAIM_TERRITORY, GUILD_PERM_INVITE, GUILD_PERM_VAULT_WITHDRAW,
    TERRITORY_PERM_ACCESS_CONTAINERS, TERRITORY_PERM_BUILD, TERRITORY_PERM_ENTER,
    TERRITORY_PERM_USE_PORTALS,
};

#[test]
fn test_guild_rank_hierarchy_and_permissions() {
    let mut guild = Guild::new(10, "Knights of the Round", "KTR", 1, 101, 100);

    // Guild master (Account 1) has all permissions
    assert!(guild.has_permission(1, GUILD_PERM_INVITE));
    assert!(guild.has_permission(1, GUILD_PERM_VAULT_WITHDRAW));
    assert!(guild.has_permission(1, GUILD_PERM_CLAIM_TERRITORY));

    // Add recruit (Account 2)
    guild.add_member(2, 202, 101).unwrap();
    // Recruit (Rank 0) has no elevated permissions
    assert!(!guild.has_permission(2, GUILD_PERM_INVITE));
    assert!(!guild.has_permission(2, GUILD_PERM_VAULT_WITHDRAW));
    assert!(!guild.has_permission(2, GUILD_PERM_CLAIM_TERRITORY));

    // Master promotes Account 2 to Veteran (Rank 2)
    guild.set_rank(1, 2, 2).unwrap();
    assert!(guild.has_permission(2, GUILD_PERM_VAULT_WITHDRAW));
    assert!(!guild.has_permission(2, GUILD_PERM_INVITE));

    // Master promotes Account 2 to Officer (Rank 3)
    guild.set_rank(1, 2, 3).unwrap();
    assert!(guild.has_permission(2, GUILD_PERM_INVITE));
    assert!(guild.has_permission(2, GUILD_PERM_VAULT_WITHDRAW));
    assert!(!guild.has_permission(2, GUILD_PERM_CLAIM_TERRITORY));

    // Add another recruit (Account 3)
    guild.add_member(3, 303, 102).unwrap();

    // Officer (Account 2, Rank 3) can promote Recruit (Account 3) to Veteran (Rank 2 < Rank 3)
    assert!(guild.set_rank(2, 3, 2).is_ok());

    // Officer (Account 2, Rank 3) CANNOT promote anyone to Officer (Rank 3) or higher!
    assert_eq!(guild.set_rank(2, 3, 3), Err(SocialError::PermissionDenied));

    // No one can demote or kick the Guild Master
    assert_eq!(
        guild.set_rank(2, 1, 0),
        Err(SocialError::CannotModifyGuildMaster)
    );
    assert_eq!(
        guild.remove_member(1),
        Err(SocialError::CannotModifyGuildMaster)
    );
}

#[test]
fn test_alliance_coalition_lifecycle() {
    let mut social = SocialManager::new();

    let guild1 = Guild::new(1, "Northern Legion", "NL", 10, 101, 1);
    let guild2 = Guild::new(2, "Southern Vanguard", "SV", 20, 202, 1);
    let guild3 = Guild::new(3, "Eastern Vanguard", "EV", 30, 303, 1);

    social.create_guild(guild1).unwrap();
    social.create_guild(guild2).unwrap();
    social.create_guild(guild3).unwrap();

    // Guild 1 founds the Grand Alliance
    let mut alliance = Alliance::new(100, "Grand Coalition", 1);
    assert!(alliance.contains_guild(1));
    assert!(!alliance.contains_guild(2));

    // Guild 2 joins
    alliance.add_guild(2).unwrap();
    assert!(alliance.contains_guild(2));

    // Duplicate join rejected
    assert_eq!(
        alliance.add_guild(2),
        Err(SocialError::GuildAlreadyInAlliance)
    );

    // Guild 2 leaves alliance
    alliance.remove_guild(2).unwrap();
    assert!(!alliance.contains_guild(2));

    // Founding guild cannot leave its own alliance
    assert_eq!(
        alliance.remove_guild(1),
        Err(SocialError::CannotModifyGuildMaster)
    );
}

#[test]
fn test_territory_spatial_containment_and_upkeep() {
    let mut manager = TerritoryManager::new();

    // Plot 500 spanning (10.0, 10.0) to (100.0, 100.0) in Sector (0, 0)
    let plot = TerritoryPlot::new(500, 0, 0, (10.0, 10.0), (100.0, 100.0));
    manager.register_plot(plot);

    // Point queries
    assert!(manager.find_plot_at(0, 0, (50.0, 50.0)).is_some());
    assert!(manager.find_plot_at(0, 0, (5.0, 50.0)).is_none());
    assert!(manager.find_plot_at(1, 0, (50.0, 50.0)).is_none());

    // Claim plot for guild 42 for 500 ticks at tick 100 (expires at tick 600)
    manager.claim_plot(500, 42, 500, 100).unwrap();

    let plot = manager.get_plot(500).unwrap();
    assert!(plot.is_claim_active(200));
    assert!(plot.is_claim_active(599));
    assert!(!plot.is_claim_active(600), "Upkeep lapsed at tick 600");

    // Refresh upkeep by 200 ticks
    let new_expiry = manager.refresh_upkeep(500, 42, 200, 200).unwrap();
    assert_eq!(new_expiry, 800); // 600 + 200 = 800
}

#[test]
fn test_territory_permission_acl_matrix() {
    let mut manager = TerritoryManager::new();
    let plot = TerritoryPlot::new(777, 0, 0, (0.0, 0.0), (128.0, 128.0));
    manager.register_plot(plot);

    let owning_guild = 100;
    let allied_guild = 200;
    let stranger_guild = 300;

    manager.claim_plot(777, owning_guild, 1000, 1).unwrap();
    let plot = manager.get_plot(777).unwrap();

    // Case 1: Owning guild member
    assert!(plot.has_permission(Some(owning_guild), false, TERRITORY_PERM_ENTER, 50));
    assert!(plot.has_permission(Some(owning_guild), false, TERRITORY_PERM_BUILD, 50));
    assert!(plot.has_permission(
        Some(owning_guild),
        false,
        TERRITORY_PERM_ACCESS_CONTAINERS,
        50
    ));

    // Case 2: Allied guild member
    assert!(plot.has_permission(Some(allied_guild), true, TERRITORY_PERM_ENTER, 50));
    assert!(plot.has_permission(Some(allied_guild), true, TERRITORY_PERM_USE_PORTALS, 50));
    assert!(!plot.has_permission(Some(allied_guild), true, TERRITORY_PERM_BUILD, 50));
    assert!(!plot.has_permission(
        Some(allied_guild),
        true,
        TERRITORY_PERM_ACCESS_CONTAINERS,
        50
    ));

    // Case 3: Stranger / non-allied player
    assert!(plot.has_permission(Some(stranger_guild), false, TERRITORY_PERM_ENTER, 50));
    assert!(!plot.has_permission(Some(stranger_guild), false, TERRITORY_PERM_BUILD, 50));
    assert!(!plot.has_permission(
        Some(stranger_guild),
        false,
        TERRITORY_PERM_ACCESS_CONTAINERS,
        50
    ));
    assert!(!plot.has_permission(Some(stranger_guild), false, TERRITORY_PERM_USE_PORTALS, 50));
}

#[test]
fn test_diplomatic_standing_matrix_and_war_declaration() {
    let mut social = SocialManager::new();
    let guild_a = 10;
    let guild_b = 20;

    // Initially neutral (0 score)
    assert_eq!(social.get_standing_score(guild_a, guild_b), 0);
    assert_eq!(
        social.get_diplomatic_status(guild_a, guild_b),
        DiplomaticStatus::Neutral
    );

    // Guild A declares war on Guild B (-10,000 score)
    social.set_standing(guild_a, guild_b, -10_000);
    assert_eq!(
        social.get_diplomatic_status(guild_a, guild_b),
        DiplomaticStatus::AtWar
    );

    // Hostility de-escalation to minor friction (-3,000 score)
    social.set_standing(guild_a, guild_b, -3000);
    assert_eq!(
        social.get_diplomatic_status(guild_a, guild_b),
        DiplomaticStatus::Hostile
    );

    // Non-aggression pact (+3,000 score)
    social.set_standing(guild_a, guild_b, 3000);
    assert_eq!(
        social.get_diplomatic_status(guild_a, guild_b),
        DiplomaticStatus::Friendly
    );

    // Mutual defense treaty (+9,000 score)
    social.set_standing(guild_a, guild_b, 9000);
    assert_eq!(
        social.get_diplomatic_status(guild_a, guild_b),
        DiplomaticStatus::Allied
    );
}
