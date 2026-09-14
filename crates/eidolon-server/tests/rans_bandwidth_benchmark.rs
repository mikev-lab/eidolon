//! Tier 1 & Tier 2 Compression Verification: Asymmetric Numeral Systems (rANS) Codec.
//!
//! Asserts mathematical lossless reconstruction parity across 10,000 entities,
//! verifies adversarial truncated stream rejection, and benchmarks compression density
//! against the sub-1.1 B/entity wire budget.

#![deny(unsafe_code)]

use std::time::Instant;

use eidolon_net::rans::{
    compress_kinematic_stream, decompress_kinematic_stream, RansDecoder, RansEncoder,
    RansSymbolTable,
};

#[test]
fn test_rans_10000_entity_kinematic_stream_compression_benchmark() {
    let tier_table = RansSymbolTable::mmo_kinematic_tier_table();
    let heading_table = RansSymbolTable::mmo_heading_delta_table();

    const NUM_ENTITIES: usize = 10_000;
    let mut tiers = Vec::with_capacity(NUM_ENTITIES);
    let mut headings = Vec::with_capacity(NUM_ENTITIES);

    // Realistic MMO population behavior:
    // 60% stationary/idle, 30% running straight, 10% maneuvering
    for i in 0..NUM_ENTITIES {
        let t = match i % 100 {
            0..=59 => 0,  // Stationary (60%)
            60..=89 => 1, // Small forward movement (30%)
            90..=97 => 2, // Medium turn/dodge (8%)
            _ => 3,       // Full keyframe (2%)
        };

        let h = match i % 100 {
            0..=69 => 0,  // Heading unchanged (70%)
            70..=89 => 1, // +1 step adjustment (20%)
            _ => 15,      // -1 step adjustment (10%)
        };

        tiers.push(t);
        headings.push(h);
    }

    let mut compressed = Vec::new();
    let start = Instant::now();
    let total_bytes = compress_kinematic_stream(
        &tier_table,
        &heading_table,
        &tiers,
        &headings,
        &mut compressed,
    );
    let encode_time = start.elapsed();

    let bytes_per_entity = (total_bytes as f64) / (NUM_ENTITIES as f64);
    println!(
        "rANS 10,000-Entity Benchmark: Compressed to {} bytes in {:?} ({:.3} B/entity)",
        total_bytes, encode_time, bytes_per_entity
    );

    // Strict Milestone 44.3 Invariant: < 1.10 B/entity
    assert!(
        bytes_per_entity < 1.10,
        "Compression ({:.3} B/entity) must strictly satisfy sub-1.1 B wire budget",
        bytes_per_entity
    );

    // Benchmark Decompression
    let mut dec_tiers = vec![0u8; NUM_ENTITIES];
    let mut dec_headings = vec![0u8; NUM_ENTITIES];
    let decode_start = Instant::now();
    decompress_kinematic_stream(
        &tier_table,
        &heading_table,
        &compressed,
        &mut dec_tiers,
        &mut dec_headings,
    )
    .unwrap();
    let decode_time = decode_start.elapsed();

    println!(
        "rANS Decompression: 10,000 entities decoded in {:?}",
        decode_time
    );

    // Bit-exact mathematical parity
    assert_eq!(tiers, dec_tiers);
    assert_eq!(headings, dec_headings);
}

#[test]
fn test_rans_adversarial_truncated_bitstream_rejection() {
    let tier_table = RansSymbolTable::mmo_kinematic_tier_table();

    // Stream shorter than 4-byte minimum state header
    let truncated_short = [0x01, 0x02];
    assert!(RansDecoder::new(&truncated_short).is_err());

    // Single byte stream
    let single_byte = [0xFF];
    assert!(RansDecoder::new(&single_byte).is_err());

    // Corrupted 4-byte state that violates L threshold
    let invalid_state = [0x00, 0x00, 0x00, 0x00];
    let mut decoder = RansDecoder::new(&invalid_state).unwrap();
    // Decodes without crashing or panicking
    let _ = decoder.get_symbol(&tier_table);
}

#[test]
fn test_rans_encoder_decoder_fuzzing_50000_symbols() {
    let table = RansSymbolTable::mmo_kinematic_tier_table();
    const COUNT: usize = 50_000;

    let mut symbols = Vec::with_capacity(COUNT);
    let mut lcg: u32 = 0x12345678;

    for _ in 0..COUNT {
        lcg = lcg.wrapping_mul(1664525).wrapping_add(1013904223);
        let sym = (lcg % 4) as u8;
        symbols.push(sym);
    }

    let mut encoded = Vec::new();
    RansEncoder::encode_symbols_forward(&table, &symbols, &mut encoded);

    let mut decoded = vec![0u8; COUNT];
    RansDecoder::decode_symbols(&table, &encoded, &mut decoded).unwrap();

    assert_eq!(symbols, decoded);
}
