//! Headless server binary for the eidolon MMO engine.
//!
//! Coordinates the 20 Hz fixed-step authoritative world simulation,
//! asynchronous network I/O, and Agones Kubernetes orchestration hooks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use eidolon_net::protocol::{PROTOCOL_MAGIC, PROTOCOL_VERSION};
use eidolon_server::agones::AgonesClient;
use eidolon_server::config::ServerConfig;
use eidolon_server::io::NetworkIoWorker;
use eidolon_server::queue::SpscPacketQueue;
use eidolon_server::tick::TickCoordinator;

/// Runs the authoritative 20 Hz simulation tick loop until `running` becomes false or `max_ticks` is reached.
pub fn run_authoritative_loop(
    worker: &NetworkIoWorker,
    ingress_queue: &SpscPacketQueue<1024>,
    egress_queue: &SpscPacketQueue<1024>,
    agones: &mut AgonesClient,
    coordinator: &mut TickCoordinator,
    running: Arc<AtomicBool>,
    max_ticks: Option<u64>,
) {
    let mut ticks_executed = 0u64;

    while running.load(Ordering::Relaxed) {
        if let Some(limit) = max_ticks {
            if ticks_executed >= limit {
                break;
            }
        }

        let tick_start = Instant::now();

        // 1. Drain ingress packets from network worker
        let _received = worker.drain_ingress(ingress_queue, 64);

        // 2. Authoritative simulation tick (fixed-step)
        // Agones periodic health check ping every 40 ticks (~2 seconds at 20 Hz)
        if ticks_executed.is_multiple_of(40) {
            let _ = agones.health();
        }

        // 3. Flush egress packets to network worker
        let _sent = worker.flush_egress(egress_queue, 64);

        let execution_duration = tick_start.elapsed();
        coordinator.record_tick_execution(execution_duration);
        coordinator.sleep_headroom(execution_duration);

        ticks_executed += 1;
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

    let running = Arc::new(AtomicBool::new(true));

    // Support optional bounded ticks via environment variable for headless CI/testing
    let max_ticks = std::env::var("EIDOLON_MAX_TICKS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok());

    println!("Starting authoritative 20 Hz simulation tick loop...");

    run_authoritative_loop(
        &worker,
        &ingress_queue,
        &egress_queue,
        &mut agones,
        &mut coordinator,
        running,
        max_ticks,
    );

    // Graceful drain and shutdown sequence
    let _ = agones.shutdown();

    let metrics = coordinator.metrics();
    println!(
        "Server shutdown complete. Total ticks: {}, Overruns: {}, Shedding level: {:?}",
        metrics.total_ticks, metrics.overrun_ticks, metrics.shedding_level
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::time::Duration;

    #[test]
    fn test_default_server_config() {
        let config = ServerConfig::default();
        assert_eq!(config.tick_rate_hz, 20);
        assert_eq!(config.default_zone.0, 1);
        assert_eq!(config.tick_interval_millis(), 50);
    }

    #[test]
    fn test_authoritative_loop_bounded_ticks() {
        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let worker = NetworkIoWorker::bind(addr).expect("bind worker");
        let ingress_queue = SpscPacketQueue::<1024>::new();
        let egress_queue = SpscPacketQueue::<1024>::new();
        let mut agones = AgonesClient::new(false, 9357);
        let mut coordinator = TickCoordinator::new(100); // 100 Hz for fast test
        let running = Arc::new(AtomicBool::new(true));

        run_authoritative_loop(
            &worker,
            &ingress_queue,
            &egress_queue,
            &mut agones,
            &mut coordinator,
            running,
            Some(5),
        );

        assert_eq!(coordinator.metrics().total_ticks, 5);
    }

    #[test]
    fn test_authoritative_loop_atomic_shutdown() {
        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let worker = NetworkIoWorker::bind(addr).expect("bind worker");
        let ingress_queue = SpscPacketQueue::<1024>::new();
        let egress_queue = SpscPacketQueue::<1024>::new();
        let mut agones = AgonesClient::new(false, 9357);
        let mut coordinator = TickCoordinator::new(100);
        let running = Arc::new(AtomicBool::new(true));

        let running_clone = running.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            running_clone.store(false, Ordering::Relaxed);
        });

        run_authoritative_loop(
            &worker,
            &ingress_queue,
            &egress_queue,
            &mut agones,
            &mut coordinator,
            running,
            None,
        );

        assert!(coordinator.metrics().total_ticks > 0);
    }
}
