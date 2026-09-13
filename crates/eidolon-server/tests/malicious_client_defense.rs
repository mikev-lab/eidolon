//! Automated test suite verifying Milestone 8.2: Malicious-Client Resource Fencing & Anti-DoS Quotas.
//!
//! Validates:
//! - Token-bucket rate policing under synthetic packet flooding (throttling 1,000 pps to 40 pps).
//! - Connection memory allocation tracking enforcing hard 64 KB ceiling per connection.
//! - Untrusted packet parsing timing verification ensuring <5 microsecond execution per datagram.

use std::time::Instant;

use eidolon_net::packet::PacketHeader;
use eidolon_net::quota::{ConnectionMemoryAccountant, SocketRatePolicer, MAX_CONNECTION_MEMORY};
use eidolon_net::BitReader;

#[test]
fn test_milestone_8_2_socket_flooding_policing() {
    let mut policer = SocketRatePolicer::new(40, 32_768);

    let mut accepted = 0;
    let mut dropped = 0;

    // Simulate burst of 1,000 incoming 100-byte packets in tick 1
    for _ in 0..1000 {
        if policer.check_ingress(100, 1).is_ok() {
            accepted += 1;
        } else {
            dropped += 1;
        }
    }

    assert_eq!(
        accepted, 40,
        "Must accept exactly 40 packets in initial bucket"
    );
    assert_eq!(dropped, 960, "Must drop exactly 960 flooding packets");

    // Advance by 1 tick (50ms) -> replenishes (40 / 20) = 2 packet tokens
    let mut tick_2_accepted = 0;
    for _ in 0..10 {
        if policer.check_ingress(100, 2).is_ok() {
            tick_2_accepted += 1;
        }
    }
    assert_eq!(
        tick_2_accepted, 2,
        "Must strictly replenish 2 tokens per 20 Hz tick"
    );
}

#[test]
fn test_milestone_8_2_connection_memory_ceiling() {
    let mut accountant = ConnectionMemoryAccountant::new(MAX_CONNECTION_MEMORY);

    // Track 60 KB in 10 KB chunks (valid)
    for _ in 0..6 {
        assert!(accountant.track_allocation(10 * 1024).is_ok());
    }
    assert_eq!(accountant.allocated_bytes(), 60 * 1024);

    // Attempting to allocate 5 KB exceeds 64 KB limit
    assert!(accountant.track_allocation(5 * 1024).is_err());
    assert_eq!(accountant.allocated_bytes(), 60 * 1024);

    // Releasing 20 KB permits subsequent 10 KB allocation
    accountant.release_allocation(20 * 1024);
    assert_eq!(accountant.allocated_bytes(), 40 * 1024);
    assert!(accountant.track_allocation(10 * 1024).is_ok());
    assert_eq!(accountant.allocated_bytes(), 50 * 1024);
}

#[test]
fn test_milestone_8_2_cpu_bounded_packet_parsing() {
    // Construct 10,000 malformed datagrams of various corruptions
    let mut malformed_packets = Vec::with_capacity(10_000);
    for i in 0..10_000 {
        let mut pkt = vec![0u8; (i % 256) + 1];
        // Inject pseudo-random noise
        for (j, byte) in pkt.iter_mut().enumerate() {
            *byte = ((i * 37 + j * 13) & 0xFF) as u8;
        }
        malformed_packets.push(pkt);
    }

    let start = Instant::now();
    let mut rejected_count = 0;

    for pkt in &malformed_packets {
        // Attempt parsing as packet header
        if PacketHeader::read_from(pkt).is_err() {
            rejected_count += 1;
        }
        // Attempt parsing bitstream varint
        let mut reader = BitReader::new(pkt);
        let _ = reader.read_varint();
    }

    let elapsed = start.elapsed();
    let avg_micros = elapsed.as_micros() as f64 / malformed_packets.len() as f64;

    println!(
        "Milestone 8.2 CPU Defense: 10,000 malformed packets parsed in {:?} (avg {:.3} µs/packet)",
        elapsed, avg_micros
    );

    assert!(rejected_count > 0);
    assert!(
        avg_micros < 5.0,
        "Untrusted packet parsing must execute in under 5.0 µs per packet (measured: {:.3} µs)",
        avg_micros
    );
}
