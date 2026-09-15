//! Territory Sovereignty, Spatial Boundary Plots & Dynamic Permissions.
//!
//! Enforces spatial territorial jurisdiction, guild claim upkeep lifecycles,
//! and Access Control List (ACL) permission bitmasks over world plots.

use std::collections::HashMap;

/// Permission to physically cross and enter the territory plot.
pub const TERRITORY_PERM_ENTER: u32 = 1 << 0;
/// Permission to place foundations or structures on the plot.
pub const TERRITORY_PERM_BUILD: u32 = 1 << 1;
/// Permission to open chests and storage containers situated on the plot.
pub const TERRITORY_PERM_ACCESS_CONTAINERS: u32 = 1 << 2;
/// Permission to harvest natural resources located on the plot.
pub const TERRITORY_PERM_HARVEST: u32 = 1 << 3;
/// Permission to activate fast-travel portals located on the plot.
pub const TERRITORY_PERM_USE_PORTALS: u32 = 1 << 4;
/// Permission to trigger automated defensive guards or turrets.
pub const TERRITORY_PERM_PVP_GUARD: u32 = 1 << 5;

/// Errors arising from territory sovereignty operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerritoryError {
    /// Plot does not exist.
    PlotNotFound,
    /// Plot is already claimed by an active guild.
    PlotAlreadyClaimed,
    /// Upkeep duration must be greater than zero.
    InvalidUpkeepDuration,
    /// Actor lacks permission to claim or modify territory.
    PermissionDenied,
}

impl core::fmt::Display for TerritoryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::PlotNotFound => write!(f, "Territory plot not found"),
            Self::PlotAlreadyClaimed => write!(f, "Territory plot is already claimed"),
            Self::InvalidUpkeepDuration => write!(f, "Invalid territory upkeep duration"),
            Self::PermissionDenied => write!(f, "Insufficient territory permissions"),
        }
    }
}

impl std::error::Error for TerritoryError {}

/// Spatial territory plot with 2D bounds and permission ACLs.
#[derive(Debug, Clone, PartialEq)]
pub struct TerritoryPlot {
    /// Unique territory plot ID.
    pub plot_id: u64,
    /// Global sector coordinate X.
    pub sector_x: i32,
    /// Global sector coordinate Z.
    pub sector_z: i32,
    /// Minimum local bounds (X, Z) within sector [0.0, 256.0].
    pub min_bounds: (f32, f32),
    /// Maximum local bounds (X, Z) within sector [0.0, 256.0].
    pub max_bounds: (f32, f32),
    /// Owning guild ID if claimed.
    pub owning_guild_id: Option<u64>,
    /// Server tick timestamp when the claim expires unless refreshed.
    pub upkeep_expires_tick: u64,
    /// Permission bitmask granted to public / non-guild members.
    pub public_permissions: u32,
    /// Permission bitmask granted to members of allied guilds.
    pub alliance_permissions: u32,
    /// Permission bitmask granted to members of the owning guild.
    pub guild_permissions: u32,
}

impl TerritoryPlot {
    /// Creates an unclaimed territory plot.
    pub fn new(
        plot_id: u64,
        sector_x: i32,
        sector_z: i32,
        min_bounds: (f32, f32),
        max_bounds: (f32, f32),
    ) -> Self {
        Self {
            plot_id,
            sector_x,
            sector_z,
            min_bounds,
            max_bounds,
            owning_guild_id: None,
            upkeep_expires_tick: 0,
            public_permissions: TERRITORY_PERM_ENTER, // Public can enter by default
            alliance_permissions: TERRITORY_PERM_ENTER | TERRITORY_PERM_USE_PORTALS,
            guild_permissions: u32::MAX, // Guild members have all rights
        }
    }

    /// Checks if a 2D local position falls within this plot's boundaries.
    pub fn contains_point(&self, sector_x: i32, sector_z: i32, pos: (f32, f32)) -> bool {
        if self.sector_x != sector_x || self.sector_z != sector_z {
            return false;
        }

        pos.0 >= self.min_bounds.0
            && pos.0 <= self.max_bounds.0
            && pos.1 >= self.min_bounds.1
            && pos.1 <= self.max_bounds.1
    }

    /// Checks whether the claim is active (not expired).
    pub fn is_claim_active(&self, current_tick: u64) -> bool {
        self.owning_guild_id.is_some() && current_tick < self.upkeep_expires_tick
    }

    /// Evaluates if an actor has a specific permission on this plot.
    pub fn has_permission(
        &self,
        actor_guild_id: Option<u64>,
        is_allied: bool,
        permission: u32,
        current_tick: u64,
    ) -> bool {
        // If plot is unclaimed or upkeep has lapsed, all actions are permitted
        if !self.is_claim_active(current_tick) {
            return true;
        }

        let Some(owner) = self.owning_guild_id else {
            return true;
        };

        let allowed_mask = if actor_guild_id == Some(owner) {
            self.guild_permissions
        } else if is_allied {
            self.alliance_permissions
        } else {
            self.public_permissions
        };

        (allowed_mask & permission) == permission
    }
}

/// Authoritative manager for territory claims and spatial permission validation.
#[derive(Debug, Default)]
pub struct TerritoryManager {
    plots: HashMap<u64, TerritoryPlot>,
}

impl TerritoryManager {
    /// Creates a new territory manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a plot in the world.
    pub fn register_plot(&mut self, plot: TerritoryPlot) {
        self.plots.insert(plot.plot_id, plot);
    }

    /// Retrieves an immutable reference to a plot.
    pub fn get_plot(&self, plot_id: u64) -> Option<&TerritoryPlot> {
        self.plots.get(&plot_id)
    }

    /// Retrieves a mutable reference to a plot.
    pub fn get_plot_mut(&mut self, plot_id: u64) -> Option<&mut TerritoryPlot> {
        self.plots.get_mut(&plot_id)
    }

    /// Finds a territory plot containing the given world sector coordinate and local offset.
    pub fn find_plot_at(
        &self,
        sector_x: i32,
        sector_z: i32,
        pos: (f32, f32),
    ) -> Option<&TerritoryPlot> {
        self.plots
            .values()
            .find(|plot| plot.contains_point(sector_x, sector_z, pos))
    }

    /// Claims a territory plot for a guild.
    pub fn claim_plot(
        &mut self,
        plot_id: u64,
        guild_id: u64,
        duration_ticks: u64,
        current_tick: u64,
    ) -> Result<(), TerritoryError> {
        if duration_ticks == 0 {
            return Err(TerritoryError::InvalidUpkeepDuration);
        }

        let plot = self
            .plots
            .get_mut(&plot_id)
            .ok_or(TerritoryError::PlotNotFound)?;

        // If currently claimed and active by another guild, reject claim
        if plot.is_claim_active(current_tick) && plot.owning_guild_id != Some(guild_id) {
            return Err(TerritoryError::PlotAlreadyClaimed);
        }

        plot.owning_guild_id = Some(guild_id);
        plot.upkeep_expires_tick = current_tick.saturating_add(duration_ticks);

        Ok(())
    }

    /// Refreshes upkeep for an already claimed plot.
    pub fn refresh_upkeep(
        &mut self,
        plot_id: u64,
        guild_id: u64,
        additional_ticks: u64,
        current_tick: u64,
    ) -> Result<u64, TerritoryError> {
        let plot = self
            .plots
            .get_mut(&plot_id)
            .ok_or(TerritoryError::PlotNotFound)?;

        if plot.owning_guild_id != Some(guild_id) {
            return Err(TerritoryError::PermissionDenied);
        }

        let base = if plot.upkeep_expires_tick > current_tick {
            plot.upkeep_expires_tick
        } else {
            current_tick
        };

        plot.upkeep_expires_tick = base.saturating_add(additional_ticks);
        Ok(plot.upkeep_expires_tick)
    }

    /// Revokes or relinquishes a guild's territory claim.
    pub fn relinquish_claim(&mut self, plot_id: u64) -> Result<(), TerritoryError> {
        let plot = self
            .plots
            .get_mut(&plot_id)
            .ok_or(TerritoryError::PlotNotFound)?;
        plot.owning_guild_id = None;
        plot.upkeep_expires_tick = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_territory_spatial_point_lookup() {
        let plot = TerritoryPlot::new(1, 0, 0, (10.0, 10.0), (50.0, 50.0));
        assert!(plot.contains_point(0, 0, (25.0, 25.0)));
        assert!(!plot.contains_point(0, 0, (5.0, 25.0)));
        assert!(!plot.contains_point(1, 0, (25.0, 25.0)));
    }

    #[test]
    fn test_territory_claim_and_acl_permissions() {
        let mut manager = TerritoryManager::new();
        let plot = TerritoryPlot::new(100, 0, 0, (0.0, 0.0), (100.0, 100.0));
        manager.register_plot(plot);

        // Claim plot for guild 42 for 1000 ticks
        manager.claim_plot(100, 42, 1000, 10).unwrap();

        let plot = manager.get_plot(100).unwrap();
        assert!(plot.is_claim_active(10));

        // Guild member can build and access containers
        assert!(plot.has_permission(Some(42), false, TERRITORY_PERM_BUILD, 10));
        assert!(plot.has_permission(Some(42), false, TERRITORY_PERM_ACCESS_CONTAINERS, 10));

        // Allied guild member can enter and use portals, but cannot build or loot containers
        assert!(plot.has_permission(Some(99), true, TERRITORY_PERM_ENTER, 10));
        assert!(!plot.has_permission(Some(99), true, TERRITORY_PERM_BUILD, 10));
        assert!(!plot.has_permission(Some(99), true, TERRITORY_PERM_ACCESS_CONTAINERS, 10));

        // Public player can enter, but cannot build or access containers
        assert!(plot.has_permission(None, false, TERRITORY_PERM_ENTER, 10));
        assert!(!plot.has_permission(None, false, TERRITORY_PERM_BUILD, 10));

        // After upkeep expires at tick 1010
        assert!(plot.has_permission(None, false, TERRITORY_PERM_BUILD, 1011));
    }
}
