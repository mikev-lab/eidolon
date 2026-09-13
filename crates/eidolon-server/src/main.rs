//! Headless server binary for the eidolon MMO engine.
//!
//! Coordinates the 20 Hz fixed-step authoritative world simulation,
//! asynchronous network I/O, and Agones Kubernetes orchestration hooks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

use eidolon_core::fixed::Vec3Fix;
use eidolon_net::protocol::{PROTOCOL_MAGIC, PROTOCOL_VERSION};
use eidolon_spatial::grid::CellCoord;
use eidolon_world::zone::ZoneId;

/// Server configuration parameters.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Target simulation tick rate in Hertz (e.g. 20 Hz = 50ms tick).
    pub tick_rate_hz: u32,
    /// Default primary world zone to simulate.
    pub default_zone: ZoneId,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            tick_rate_hz: 20,
            default_zone: ZoneId(1),
        }
    }
}

fn main() {
    let config = ServerConfig::default();
    println!(
        "eidolon-server v{} initializing [magic: {:?}, proto: v{}]",
        env!("CARGO_PKG_VERSION"),
        PROTOCOL_MAGIC,
        PROTOCOL_VERSION
    );
    println!(
        "Simulation target: {} Hz (tick interval: {}ms)",
        config.tick_rate_hz,
        1000 / config.tick_rate_hz
    );

    // Assert initial zero constants for compiler verification
    let _origin = Vec3Fix::ZERO;
    let _root_cell = CellCoord::new(0, 0, 0);
}

#[cfg(test)]
mod tests {
    use super::ServerConfig;

    #[test]
    fn test_default_server_config() {
        let config = ServerConfig::default();
        assert_eq!(config.tick_rate_hz, 20);
        assert_eq!(config.default_zone.0, 1);
    }
}
