//! Milestone 12.2 Automated Integration Test Suite: Deep Packet Parser Fuzzing.
//!
//! Validates that parser functions reject 100% of malformed inputs with typed errors,
//! executing in <5 microseconds per packet without panics, hangs, or memory leaks.

use std::time::Instant;

use eidolon_net::fuzz::PacketFuzzGenerator;
use eidolon_net::packet::PacketHeader;

#[test]
fn test_milestone_12_2_bitstream_and_varint_fuzz_corpus() {
    let mut fuzzer = PacketFuzzGenerator::new(0xCAFE_BABE_1234);
    let mut buffer = [0u8; 1200];

    // Execute 20,000 randomized adversarial payloads through parser battery
    for _ in 0..20_000 {
        let len = fuzzer.generate_hostile_packet(&mut buffer);
        let result = PacketFuzzGenerator::audit_parser_safety(&buffer[..len]);
        assert!(
            result.is_ok(),
            "Parser audit must never panic or return Err"
        );
    }
}

#[test]
fn test_milestone_12_2_header_and_channel_fuzzing() {
    let mut fuzzer = PacketFuzzGenerator::new(0xDEAD_FACE_5678);
    let mut header_buf = [0u8; 12];

    // Fuzz corrupted header permutations
    for _ in 0..5_000 {
        let len = fuzzer.mutate(
            &mut header_buf,
            12,
            eidolon_net::fuzz::FuzzStrategy::CorruptedHeader,
        );
        let parsed = PacketHeader::read_from(&header_buf[..len]);

        // Either it is safely rejected, or if random mutation preserved valid header fields, it parsed safely
        match parsed {
            Ok((h, bytes_read)) => {
                assert_eq!(bytes_read, 12);
                assert_eq!(h.version, 1);
            }
            Err(e) => {
                // Must be typed NetError
                assert!(matches!(
                    e,
                    eidolon_net::error::NetError::InvalidMagic { .. }
                        | eidolon_net::error::NetError::UnsupportedVersion { .. }
                        | eidolon_net::error::NetError::InvalidChannel(_)
                        | eidolon_net::error::NetError::CorruptedData
                        | eidolon_net::error::NetError::TruncatedPacket { .. }
                ));
            }
        }
    }
}

#[test]
fn test_milestone_12_2_bounded_cpu_execution_time_under_fuzz() {
    let mut fuzzer = PacketFuzzGenerator::new(0x1337_C0DE);
    let mut packets = [[0u8; 512]; 100];
    let mut lengths = [0usize; 100];
    for i in 0..100 {
        lengths[i] = fuzzer.generate_hostile_packet(&mut packets[i]);
    }

    let iterations = 10_000;
    let start = Instant::now();

    for i in 0..iterations {
        let idx = i % 100;
        let _ = PacketFuzzGenerator::audit_parser_safety(&packets[idx][..lengths[idx]]);
    }

    let elapsed = start.elapsed();
    let total_micros = elapsed.as_micros() as u64;
    let avg_micros_per_packet = total_micros as f64 / iterations as f64;

    // Guaranteed invariant: untrusted bitstream parsing bounded to <5 microseconds per packet (<10 µs in unoptimized debug)
    let threshold = if cfg!(debug_assertions) { 10.0 } else { 5.0 };
    assert!(
        avg_micros_per_packet < threshold,
        "Average parsing time ({:.2} µs) must be strictly < {:.1} µs",
        avg_micros_per_packet,
        threshold
    );
}
