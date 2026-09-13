//! Server configuration parameters and default runtime limits.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use eidolon_world::error::ZoneId;

/// Authoritative server configuration parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// UDP socket bind address.
    pub bind_addr: SocketAddr,
    /// Authoritative simulation tick rate in Hertz (default 20 Hz = 50ms tick).
    pub tick_rate_hz: u32,
    /// Maximum concurrent connected players.
    pub max_players: usize,
    /// Indicates whether Agones Kubernetes orchestration hooks are enabled.
    pub agones_enabled: bool,
    /// Agones sidecar local HTTP port (default 9358).
    pub agones_port: u16,
    /// Default primary world zone to simulate.
    pub default_zone: ZoneId,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7777),
            tick_rate_hz: 20,
            max_players: 2000,
            agones_enabled: false,
            agones_port: 9358,
            default_zone: ZoneId(1),
        }
    }
}

impl ServerConfig {
    /// Creates a new server configuration with custom bind address and tick rate.
    pub fn new(bind_addr: SocketAddr, tick_rate_hz: u32, max_players: usize) -> Self {
        Self {
            bind_addr,
            tick_rate_hz: tick_rate_hz.max(1),
            max_players,
            ..Default::default()
        }
    }

    /// Returns the target tick duration in microseconds.
    #[inline]
    pub fn tick_interval_micros(&self) -> u64 {
        1_000_000 / (self.tick_rate_hz as u64)
    }

    /// Returns the target tick duration in milliseconds.
    #[inline]
    pub fn tick_interval_millis(&self) -> u64 {
        1_000 / (self.tick_rate_hz as u64)
    }
}
