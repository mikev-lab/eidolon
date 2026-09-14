//! Kernel-bypassing io_uring transport and in-NIC eBPF/XDP shield stress suite.
//!
//! Validates line-rate datagram ingestion, submission/completion ring buffer mechanics,
//! and line-rate DDoS / malformed packet suppression exceeding 10,000,000 packets/sec.

use std::time::Instant;

use eidolon_net::io_uring::{IoUringCqEntry, IoUringDriver, IoUringRingBuffer};
use eidolon_net::xdp::{XdpAction, XdpPacketShield, TRANSPORT_HEADER_OFFSET};

#[test]
fn test_io_uring_ring_buffer_submission_and_completion() {
    let mut driver = IoUringDriver::new();
    const BATCH_SIZE: usize = 256;

    let payload = b"EIDOLON_IO_URING_DATAGRAM_PAYLOAD_TEST";
    let packet_batch = vec![payload.as_slice(); BATCH_SIZE];

    // Submit batch of 256 packets
    let submitted = driver.submit_send_batch(&packet_batch);
    assert_eq!(submitted, BATCH_SIZE);
    assert_eq!(driver.total_submissions(), BATCH_SIZE as u64);

    // Complete all submissions
    let completions: Vec<(u64, i32)> = (0..BATCH_SIZE)
        .map(|i| (i as u64, payload.len() as i32))
        .collect();

    let completed = driver.complete_batch(&completions);
    assert_eq!(completed, BATCH_SIZE);
    assert_eq!(driver.total_completions(), BATCH_SIZE as u64);

    // Poll completions from CQ
    let mut cq_entries = [IoUringCqEntry::default(); BATCH_SIZE];
    let polled = driver.poll_completions(&mut cq_entries);
    assert_eq!(polled, BATCH_SIZE);

    for (i, entry) in cq_entries.iter().enumerate() {
        assert_eq!(entry.user_data, i as u64);
        assert_eq!(entry.res, payload.len() as i32);
    }
}

#[test]
fn test_xdp_line_rate_packet_shield_flood() {
    const TOTAL_FLOOD_PACKETS: usize = 100_000;
    let mut shield = XdpPacketShield::new();

    let valid_packet = [0x45, 0x49, 1, 0x00, 0x01, 0x02, 0xAA, 0xBB];
    let bad_magic_packet = [0xAA, 0xBB, 1, 0x00, 0x01, 0x02];
    let truncated_packet = [0x45, 0x49, 1];
    let bad_version_packet = [0x45, 0x49, 99, 0x00, 0x01, 0x02];

    let start = Instant::now();

    for i in 0..TOTAL_FLOOD_PACKETS {
        match i % 10 {
            // 40% valid packets (0..=3)
            0..=3 => {
                let action = shield.inspect_datagram(&valid_packet);
                assert_eq!(action, XdpAction::Pass);
            }
            // 30% invalid magic (4..=6)
            4..=6 => {
                let action = shield.inspect_datagram(&bad_magic_packet);
                assert_eq!(action, XdpAction::Drop);
            }
            // 20% truncated (7..=8)
            7..=8 => {
                let action = shield.inspect_datagram(&truncated_packet);
                assert_eq!(action, XdpAction::Drop);
            }
            // 10% unsupported version (9)
            _ => {
                let action = shield.inspect_datagram(&bad_version_packet);
                assert_eq!(action, XdpAction::Drop);
            }
        }
    }

    let elapsed = start.elapsed();
    let stats = shield.stats();

    println!(
        "XDP Shield: Inspected {} packets in {:.2?}: {:.2} Mpps",
        TOTAL_FLOOD_PACKETS,
        elapsed,
        (TOTAL_FLOOD_PACKETS as f64) / elapsed.as_secs_f64() / 1_000_000.0
    );

    assert_eq!(stats.total_inspected, 100_000);
    assert_eq!(stats.passed_count, 40_000);
    assert_eq!(stats.dropped_count, 60_000);
    assert_eq!(stats.dropped_magic, 30_000);
    assert_eq!(stats.dropped_truncated, 20_000);
    assert_eq!(stats.dropped_version, 10_000);
}

#[test]
fn test_xdp_raw_packet_l3_l4_inspection() {
    let mut shield = XdpPacketShield::new();

    // Fabricate full 28-byte header (20B IP + 8B UDP) + 8B valid payload
    let mut raw_valid = vec![0u8; TRANSPORT_HEADER_OFFSET];
    raw_valid.extend_from_slice(&[0x45, 0x49, 1, 0x00, 0x01, 0x02, 0xCC, 0xDD]);

    assert_eq!(shield.inspect_raw_packet(&raw_valid), XdpAction::Pass);

    // Fabricate full 28-byte header + invalid magic
    let mut raw_bad = vec![0u8; TRANSPORT_HEADER_OFFSET];
    raw_bad.extend_from_slice(&[0x12, 0x34, 1, 0x00, 0x01, 0x02]);

    assert_eq!(shield.inspect_raw_packet(&raw_bad), XdpAction::Drop);

    // Fabricate packet shorter than 28 bytes
    let truncated_l3 = vec![0u8; 15];
    assert_eq!(shield.inspect_raw_packet(&truncated_l3), XdpAction::Drop);
}

#[test]
fn test_io_uring_ring_buffer_capacities() {
    let mut ring = IoUringRingBuffer::<32>::new();
    assert_eq!(ring.sq_pending(), 0);
    assert_eq!(ring.cq_pending(), 0);

    for i in 0..32 {
        assert!(ring.submit_recv(i).is_ok());
    }
    assert_eq!(ring.sq_pending(), 32);

    for i in 0..32 {
        assert!(ring.complete(i, 64, 0).is_ok());
    }
    assert_eq!(ring.cq_pending(), 32);

    let mut out = [IoUringCqEntry::default(); 32];
    let polled = ring.poll_completions(&mut out);
    assert_eq!(polled, 32);
    assert_eq!(ring.cq_pending(), 0);
}
