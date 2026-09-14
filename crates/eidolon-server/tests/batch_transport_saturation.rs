//! Automated chaos and saturation test suite for Phase 37: Kernel-Bypassing Batch Transport.
//!
//! Validates:
//! - Multi-packet vectorized UDP batching (`DatagramBatch<64>`).
//! - Sustained high-throughput flood ingestion with bounded memory ceilings.
//! - Priority traffic shaping under backpressure, preserving 100% of reliable control packets.
//! - Accurate packets-per-second and bytes-per-second monitoring via `PacketRateMonitor`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use eidolon_net::batch_io::{DatagramBatch, PacketRateMonitor};
use eidolon_server::io::NetworkIoWorker;
use eidolon_server::queue::{NetworkPacket, SpscPacketQueue};

#[test]
fn test_batch_transport_saturation_sustained_flood() {
    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let worker = NetworkIoWorker::bind(server_addr).expect("bind worker");
    let bound_addr = worker.local_addr().expect("local addr");

    // Spawn 2 client sockets to flood the server simultaneously
    let client1 =
        UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).expect("client1 bind");
    let client2 =
        UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).expect("client2 bind");

    let ingress_queue = SpscPacketQueue::<1024>::new();
    let mut batch = DatagramBatch::<64>::new();
    let mut rate_monitor = PacketRateMonitor::new(1);

    let total_flood_packets = 256;
    let payload = [0x45, 0x49, 0, 0x01, 42, 99]; // 6 bytes

    // Client sockets pump packets
    for _ in 0..(total_flood_packets / 2) {
        let _ = client1.send_to(&payload, bound_addr);
        let _ = client2.send_to(&payload, bound_addr);
    }

    // Server drains in 64-packet vectorized batches
    let mut enqueued = 0;
    for _ in 0..100 {
        let drained = worker.drain_ingress_batch(&ingress_queue, &mut batch);
        enqueued += drained;
        rate_monitor.record(drained as u64, (drained * payload.len()) as u64);
        if enqueued >= total_flood_packets {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    assert!(
        enqueued >= total_flood_packets,
        "All flooded datagrams must be ingested via batch I/O, got {} of {}",
        enqueued,
        total_flood_packets
    );
    assert_eq!(ingress_queue.len(), total_flood_packets);
    assert_eq!(rate_monitor.total_packets(), total_flood_packets as u64);
    assert_eq!(
        rate_monitor.total_bytes(),
        (total_flood_packets * payload.len()) as u64
    );

    // Drain all packets and verify payload integrity
    for _ in 0..total_flood_packets {
        let pkt = ingress_queue.try_pop().expect("pop packet");
        assert_eq!(&pkt.payload[..pkt.len], &payload);
    }
    assert_eq!(ingress_queue.len(), 0);
}

#[test]
fn test_batch_transport_prioritized_traffic_shaping() {
    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let worker = NetworkIoWorker::bind(server_addr).expect("bind worker");
    let bound_addr = worker.local_addr().expect("local addr");

    let client =
        UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).expect("client bind");

    // Queue capacity 64, pre-fill with 55 packets (>80% capacity threshold = 51)
    let ingress_queue = SpscPacketQueue::<64>::new();
    let dummy_packet = NetworkPacket::new(bound_addr, &[0x45, 0x49, 0, 0, 1]).expect("dummy pkt");
    for _ in 0..55 {
        ingress_queue.try_push(dummy_packet);
    }
    assert_eq!(ingress_queue.len(), 55);

    // Send 10 unreliable movement packets (channel byte 3 == 0x00)
    // and 5 reliable control packets (channel byte 3 == 0x01)
    let unreliable_pkt = [0x45, 0x49, 0, 0x00, 10, 20];
    let reliable_pkt = [0x45, 0x49, 0, 0x01, 30, 40];

    for _ in 0..10 {
        client
            .send_to(&unreliable_pkt, bound_addr)
            .expect("send unreliable");
    }
    for _ in 0..5 {
        client
            .send_to(&reliable_pkt, bound_addr)
            .expect("send reliable");
    }

    let mut batch = DatagramBatch::<32>::new();
    let mut total_enqueued = 0;
    let mut total_shed = 0;

    for _ in 0..50 {
        let (enq, shed) = worker.drain_ingress_batch_prioritized(&ingress_queue, &mut batch);
        total_enqueued += enq;
        total_shed += shed;
        if total_enqueued + total_shed >= 15 {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    // All 10 unreliable packets must be shed under backpressure (>80% occupancy)
    // All 5 reliable packets must be accepted and enqueued
    assert_eq!(
        total_shed, 10,
        "All 10 unreliable packets must be shed under backpressure"
    );
    assert_eq!(
        total_enqueued, 5,
        "All 5 reliable control packets must be admitted"
    );
    assert_eq!(ingress_queue.len(), 60);

    // Drain the 55 pre-filled packets
    for _ in 0..55 {
        let _ = ingress_queue.try_pop();
    }

    // Verify the remaining 5 packets are strictly the reliable control packets
    for _ in 0..5 {
        let pkt = ingress_queue.try_pop().expect("reliable packet pop");
        assert_eq!(&pkt.payload[..pkt.len], &reliable_pkt);
    }
    assert_eq!(ingress_queue.len(), 0);
}

#[test]
fn test_packet_rate_monitor_live_traffic() {
    let mut monitor = PacketRateMonitor::new(20);

    // Simulate 4 successive ticks of 100 packets and 12,000 bytes each
    for _ in 0..4 {
        monitor.record(100, 12_000);
    }

    // Before window expires (e.g. at tick 10)
    let (pps_mid, bps_mid) = monitor.sample_tick(10);
    assert_eq!(pps_mid, 0);
    assert_eq!(bps_mid, 0);

    // Cross window boundary at tick 20
    let (pps_final, bps_final) = monitor.sample_tick(20);
    assert_eq!(pps_final, 400); // 400 packets over 20 ticks = 400 pps at 20 Hz
    assert_eq!(bps_final, 48_000);
    assert_eq!(monitor.total_packets(), 400);
    assert_eq!(monitor.total_bytes(), 48_000);
}
