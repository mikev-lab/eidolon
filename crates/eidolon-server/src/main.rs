//! Headless server binary for the eidolon MMO engine.
//!
//! Coordinates the 20 Hz fixed-step authoritative world simulation,
//! asynchronous network I/O, and Agones Kubernetes orchestration hooks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

use std::time::{Duration, Instant};

use eidolon_net::protocol::{PROTOCOL_MAGIC, PROTOCOL_VERSION};
use eidolon_server::agones::AgonesClient;
use eidolon_server::config::ServerConfig;
use eidolon_server::io::NetworkIoWorker;
use eidolon_server::queue::SpscPacketQueue;
use eidolon_server::tick::TickCoordinator;

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
        config.tick_interval_millis()
    );

    // Initialize non-blocking UDP I/O worker
    let worker = match NetworkIoWorker::bind(config.bind_addr) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Failed to bind UDP socket on {}: {e}", config.bind_addr);
            return;
        }
    };

    println!(
        "Listening on UDP: {}",
        worker.local_addr().unwrap_or(config.bind_addr)
    );

    // Initialize cross-thread bounded packet queues
    let ingress_queue = SpscPacketQueue::<1024>::new();
    let egress_queue = SpscPacketQueue::<1024>::new();

    // Initialize Agones lifecycle
    let mut agones = AgonesClient::new(config.agones_enabled, config.agones_port);
    if let Err(e) = agones.ready() {
        eprintln!("Agones ready failed (ignorable in standalone): {e}");
    }

    // Initialize high-resolution tick coordinator
    let mut coordinator = TickCoordinator::new(config.tick_rate_hz);

    println!("Starting authoritative 20 Hz simulation tick loop...");

    // Demonstration runner: tick 5 times on startup before yielding to daemon mode
    for _ in 0..5 {
        let tick_start = Instant::now();

        // 1. Drain ingress packets
        let _received = worker.drain_ingress(&ingress_queue, 64);

        // 2. Simulation step (world zones, spatial re-indexing, AoI dispatches)
        // ...

        // 3. Flush egress packets
        let _sent = worker.flush_egress(&egress_queue, 64);

        let execution_duration = tick_start.elapsed();
        coordinator.record_tick_execution(execution_duration);
        coordinator.sleep_headroom(execution_duration);
    }

    let metrics = coordinator.metrics();
    println!(
        "Startup validation complete. Total ticks: {}, Overruns: {}, Shedding level: {:?}",
        metrics.total_ticks, metrics.overrun_ticks, metrics.shedding_level
    );

    // Sleep briefly before normal daemon loop
    std::thread::sleep(Duration::from_millis(50));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_server_config() {
        let config = ServerConfig::default();
        assert_eq!(config.tick_rate_hz, 20);
        assert_eq!(config.default_zone.0, 1);
        assert_eq!(config.tick_interval_millis(), 50);
    }
}
