//! Milestone 13.3 Automated Integration Test Suite: Physical Wire & PCAP Accounting.
//!
//! Validates zero-dependency PCAP (libpcap 2.4) binary capture generation, Layer 1 through
//! Layer 7 physical frame accounting, and empirical proof of the <1.2 KB/s wire budget.

use eidolon_net::pcap::{
    PcapWriter, ETHERNET_HEADER_LEN, IPV4_HEADER_LEN, LINKTYPE_ETHERNET, PCAP_MAGIC_NUMBER,
    UDP_HEADER_LEN,
};
use eidolon_net::wire_accounting::{
    PhysicalFrameBreakdown, SpatialBandwidthProfile, CLOUD_EGRESS_FRAMING_OVERHEAD,
    L1_PHYSICAL_OVERHEAD, L2_ETHERNET_OVERHEAD, L3_IPV4_OVERHEAD, L4_UDP_OVERHEAD,
    L7_PROTOCOL_OVERHEAD, TOTAL_PHYSICAL_OVERHEAD,
};

#[test]
fn test_milestone_13_3_pcap_binary_file_generation_and_headers() {
    let mut buffer = Vec::new();
    let mut pcap = PcapWriter::new(&mut buffer).expect("PCAP initialization must succeed");

    assert_eq!(pcap.packets_written(), 0);
    assert_eq!(pcap.bytes_written(), 24);

    let src_ip = [10, 0, 0, 1];
    let dst_ip = [10, 0, 0, 2];
    let src_port = 7777;
    let dst_port = 8888;

    // Emit 50 packets representing 2.5 seconds of 20 Hz simulation streaming
    for tick in 1..=50 {
        let timestamp_micros = tick * 50_000;
        let payload = format!("eidolon_tick_{tick}");
        let written = pcap
            .write_udp_packet(
                timestamp_micros,
                src_ip,
                dst_ip,
                src_port,
                dst_port,
                payload.as_bytes(),
            )
            .expect("UDP frame write must succeed");

        assert_eq!(
            written,
            ETHERNET_HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN + payload.len()
        );
    }

    assert_eq!(pcap.packets_written(), 50);
    pcap.flush().expect("Flush must succeed");

    // Verify PCAP binary structure
    assert!(buffer.len() > 24);

    // Global Header verification
    let magic = u32::from_ne_bytes(buffer[0..4].try_into().unwrap());
    assert_eq!(magic, PCAP_MAGIC_NUMBER);

    let major = u16::from_ne_bytes(buffer[4..6].try_into().unwrap());
    let minor = u16::from_ne_bytes(buffer[6..8].try_into().unwrap());
    assert_eq!(major, 2);
    assert_eq!(minor, 4);

    let linktype = u32::from_ne_bytes(buffer[20..24].try_into().unwrap());
    assert_eq!(linktype, LINKTYPE_ETHERNET);

    // Verify First Packet Record Header at offset 24
    let ts_sec = u32::from_ne_bytes(buffer[24..28].try_into().unwrap());
    let ts_usec = u32::from_ne_bytes(buffer[28..32].try_into().unwrap());
    assert_eq!(ts_sec, 0);
    assert_eq!(ts_usec, 50_000); // 50ms = 50,000 µs
}

#[test]
fn test_milestone_13_3_physical_l1_to_l7_wire_reconciliation() {
    // Assert structural protocol constants
    assert_eq!(L1_PHYSICAL_OVERHEAD, 20); // 7B preamble + 1B SFD + 12B IFG
    assert_eq!(L2_ETHERNET_OVERHEAD, 18); // 14B MAC + 4B CRC32 FCS
    assert_eq!(L3_IPV4_OVERHEAD, 20); // 20B standard IPv4 header
    assert_eq!(L4_UDP_OVERHEAD, 8); // 8B standard UDP header
    assert_eq!(L7_PROTOCOL_OVERHEAD, 12); // 12B eidolon header
    assert_eq!(CLOUD_EGRESS_FRAMING_OVERHEAD, 28); // 20B IPv4 + 8B UDP
    assert_eq!(TOTAL_PHYSICAL_OVERHEAD, 78); // Sum of all lower layers + L7 header

    // 1. Standard 7-byte Intra-Cell Transform Update
    let transform_frame = PhysicalFrameBreakdown::compute(7);
    assert_eq!(transform_frame.l1_bytes, 20);
    assert_eq!(transform_frame.l2_bytes, 18);
    assert_eq!(transform_frame.l3_bytes, 20);
    assert_eq!(transform_frame.l4_bytes, 8);
    assert_eq!(transform_frame.l7_header_bytes, 12);
    assert_eq!(transform_frame.payload_bytes, 7);

    // Total physical bytes on wire: 20 + 18 + 20 + 8 + 12 + 7 = 85 bytes
    assert_eq!(transform_frame.total_physical_bytes(), 85);
    // Cloud provider billable egress (L3 IPv4 + L4 UDP + L7 payload): 20 + 8 + 12 + 7 = 47 bytes
    assert_eq!(transform_frame.cloud_egress_bytes(), 47);

    // 2. 17-byte Cell-Anchor Transform Update (AoI entry or cell crossing)
    let anchor_frame = PhysicalFrameBreakdown::compute(17);
    assert_eq!(anchor_frame.total_physical_bytes(), 95);
    assert_eq!(anchor_frame.cloud_egress_bytes(), 57);
}

#[test]
fn test_milestone_13_3_tiered_aoi_wire_budget_under_1_2kb_sec() {
    let profile = SpatialBandwidthProfile::default();
    let summary = profile.compute_coalesced_bandwidth(20.0);

    // Assert that egress adheres to the sub-1.2 KB/s (1,228.8 B/s) mandate
    assert!(
        summary.cloud_egress_bytes_per_sec < 1_200.0,
        "Cloud egress ({:.2} B/s) must strictly satisfy <1.2 KB/s budget",
        summary.cloud_egress_bytes_per_sec
    );

    // Specifically verify it hovers around the ~1,032 B/s (~1.02 KB/s) architecture target
    assert!(
        summary.cloud_egress_bytes_per_sec >= 900.0
            && summary.cloud_egress_bytes_per_sec <= 1_150.0,
        "Cloud egress ({:.2} B/s) must conform to nominal 1.02 KB/s profile",
        summary.cloud_egress_bytes_per_sec
    );

    // Physical wire including Ethernet MAC, CRC32, and preamble must remain bounded
    assert!(
        summary.physical_wire_bytes_per_sec < 2_000.0,
        "Physical wire bandwidth ({:.2} B/s) must remain bounded",
        summary.physical_wire_bytes_per_sec
    );
    assert!(summary.packets_per_second <= 20.0);
}
