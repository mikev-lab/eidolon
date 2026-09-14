//! Headless server binary for the eidolon MMO engine.
//!
//! Coordinates the 20 Hz fixed-step authoritative world simulation,
//! asynchronous network I/O, and Agones Kubernetes orchestration hooks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use eidolon_net::packet::{PacketHeader, PacketView};
use eidolon_net::protocol::{
    ChannelType, PacketType, HEADER_SIZE, PROTOCOL_MAGIC, PROTOCOL_VERSION,
};
use eidolon_server::agones::AgonesClient;
use eidolon_server::config::ServerConfig;
use eidolon_server::io::NetworkIoWorker;
use eidolon_server::queue::{NetworkPacket, SpscPacketQueue};
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
        // Ingest and validate incoming client packets zero-copy
        while let Some(packet) = ingress_queue.try_pop() {
            if let Ok(view) = PacketView::from_bytes(&packet.payload[..packet.len]) {
                match view.header.packet_type {
                    PacketType::StateUpdate => {
                        // Authoritative client movement/intent update
                    }
                    PacketType::Heartbeat => {
                        // Echo heartbeat keepalive back to client peer
                        let mut resp = [0u8; HEADER_SIZE];
                        let header = PacketHeader::new(
                            ChannelType::UnreliableSequenced,
                            PacketType::Heartbeat,
                            view.header.sequence,
                            0,
                            0,
                        );
                        if header.write_to(&mut resp).is_ok() {
                            if let Some(pkt) = NetworkPacket::new(packet.peer_addr, &resp) {
                                let _ = egress_queue.try_push(pkt);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

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

/// Runs a turnkey local 3-zone cluster on loopback UDP sockets.
pub fn run_local_cluster(running: Arc<AtomicBool>, max_ticks: Option<u64>) {
    use eidolon_server::multi_process::RealSocketZoneNode;
    use std::thread;
    use std::time::Duration;

    println!("============================================================");
    println!("  EIDOLON TURNKEY LOCAL MULTI-ZONE CLUSTER INITIALIZING     ");
    println!("============================================================");

    let mut node1 = match RealSocketZoneNode::bind(0, 1, None::<&str>) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("Failed to bind Zone 1: {e}");
            return;
        }
    };
    let mut node2 = match RealSocketZoneNode::bind(0, 2, None::<&str>) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("Failed to bind Zone 2: {e}");
            return;
        }
    };
    let mut node3 = match RealSocketZoneNode::bind(0, 3, None::<&str>) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("Failed to bind Gateway: {e}");
            return;
        }
    };

    let addr1 = node1.local_addr();
    let addr2 = node2.local_addr();
    let addr3 = node3.local_addr();

    node1.add_peer_route(2, addr2);
    node1.add_peer_route(3, addr3);

    node2.add_peer_route(1, addr1);
    node2.add_peer_route(3, addr3);

    node3.add_peer_route(1, addr1);
    node3.add_peer_route(2, addr2);

    println!("  Zone 1 [Whispering Plains] -> UDP {addr1}");
    println!("  Zone 2 [Obsidian Crags]    -> UDP {addr2}");
    println!("  Zone 3 [Nexus Gateway]     -> UDP {addr3}");
    println!("  Topology: Full mesh (3 nodes, 6 bidirectional routes)");
    println!("============================================================");

    node1.spawn_entity(1001, b"player_1_state".to_vec());
    node1.spawn_entity(1002, b"player_2_state".to_vec());
    node2.spawn_entity(2001, b"npc_dragon_state".to_vec());

    let running_1 = running.clone();
    let running_2 = running.clone();
    let running_3 = running.clone();

    let h1 = thread::spawn(move || {
        let mut ticks = 0u64;
        while running_1.load(Ordering::Relaxed) {
            if let Some(limit) = max_ticks {
                if ticks >= limit {
                    break;
                }
            }
            let _ = node1.poll_network();
            node1.check_migration_timeouts(Duration::from_millis(500));
            thread::sleep(Duration::from_millis(10));
            ticks += 1;
        }
        ticks
    });

    let h2 = thread::spawn(move || {
        let mut ticks = 0u64;
        while running_2.load(Ordering::Relaxed) {
            if let Some(limit) = max_ticks {
                if ticks >= limit {
                    break;
                }
            }
            let _ = node2.poll_network();
            node2.check_migration_timeouts(Duration::from_millis(500));
            thread::sleep(Duration::from_millis(10));
            ticks += 1;
        }
        ticks
    });

    let h3 = thread::spawn(move || {
        let mut ticks = 0u64;
        while running_3.load(Ordering::Relaxed) {
            if let Some(limit) = max_ticks {
                if ticks >= limit {
                    break;
                }
            }
            let _ = node3.poll_network();
            node3.check_migration_timeouts(Duration::from_millis(500));
            thread::sleep(Duration::from_millis(10));
            ticks += 1;
        }
        ticks
    });

    let t1 = h1.join().unwrap_or(0);
    let t2 = h2.join().unwrap_or(0);
    let t3 = h3.join().unwrap_or(0);

    println!("Local multi-zone cluster shutdown cleanly. Ticks: Z1={t1}, Z2={t2}, Z3={t3}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "cluster" || a == "--cluster") {
        let running = Arc::new(AtomicBool::new(true));
        let max_ticks = std::env::var("EIDOLON_MAX_TICKS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok());
        run_local_cluster(running, max_ticks);
        return;
    }

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

    #[test]
    fn test_run_local_cluster_bounded_ticks() {
        let running = Arc::new(AtomicBool::new(true));
        // Run cluster for 3 ticks across all 3 zones
        run_local_cluster(running, Some(3));
    }
}
