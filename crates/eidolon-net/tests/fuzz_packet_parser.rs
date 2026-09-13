//! Tier 2: Malicious and Hostile Packet Fuzzer Test Suite.
//!
//! Fuzzes 50,000+ randomly mutated, truncated, corrupted, and adversarial payloads
//! asserting that the engine never panics and always returns structured typed errors.

use eidolon_net::bitstream::{BitReader, BitWriter};
use eidolon_net::packet::{PacketHeader, PacketView};
use eidolon_net::protocol::{MAX_PACKET_SIZE, PROTOCOL_MAGIC};

/// Lightweight deterministic Xorshift PRNG for repeatable fuzz testing without external dependencies.
struct FuzzPrng {
    state: u64,
}

impl FuzzPrng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0xdeadbeefcafe1234 } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            let val = self.next_u64().to_le_bytes();
            let len = chunk.len();
            chunk.copy_from_slice(&val[..len]);
        }
    }
}

#[test]
fn test_fuzz_packet_header_parser_50k_iterations() {
    let mut rng = FuzzPrng::new(0x1337c0de);
    let mut buffer = [0u8; 128];

    let iterations = 50_000;
    let mut accepted_count = 0;
    let mut rejected_count = 0;

    for i in 0..iterations {
        // Vary length from 0 to 64 bytes
        let len = (rng.next_u64() % 65) as usize;
        rng.fill_bytes(&mut buffer[..len]);

        // In 10% of cases, insert valid magic and version to test deeper fields
        if i % 10 == 0 && len >= 4 {
            buffer[0] = PROTOCOL_MAGIC[0];
            buffer[1] = PROTOCOL_MAGIC[1];
            buffer[2] = 1; // version 1
        }

        let slice = &buffer[..len];
        match PacketHeader::read_from(slice) {
            Ok(_) => accepted_count += 1,
            Err(_) => rejected_count += 1,
        }
    }

    // Must not have panicked, and almost all random garbage must be rejected
    assert!(rejected_count > 45_000);
    assert_eq!(accepted_count + rejected_count, iterations);
}

#[test]
fn test_fuzz_packet_view_with_oversized_payloads() {
    let mut rng = FuzzPrng::new(0xfeedface);
    let mut buffer = vec![0u8; MAX_PACKET_SIZE + 500];

    for _ in 0..1_000 {
        // Generate valid header
        buffer[0] = PROTOCOL_MAGIC[0];
        buffer[1] = PROTOCOL_MAGIC[1];
        buffer[2] = 1; // version 1
        buffer[3] = 0; // UnreliableSequenced
        rng.fill_bytes(&mut buffer[4..12]);

        // Test with length exceeding MTU (MAX_PACKET_SIZE = 1200)
        let oversized_len = MAX_PACKET_SIZE + 1 + (rng.next_u64() % 400) as usize;
        let result = PacketView::from_bytes(&buffer[..oversized_len]);

        assert!(
            result.is_err(),
            "Oversized packet must be rejected before processing!"
        );
    }
}

#[test]
fn test_fuzz_bitstream_reader_random_queries() {
    let mut rng = FuzzPrng::new(0xabcdef01);
    let mut buffer = [0u8; 64];

    for _ in 0..10_000 {
        rng.fill_bytes(&mut buffer);
        let mut reader = BitReader::new(&buffer);

        // Perform random bit-width reads
        while reader.remaining_bits() > 0 {
            let width = (rng.next_u64() % 65) as usize;
            let _ = reader.read_bits(width);
        }

        // Must gracefully handle reads when stream is fully consumed
        assert!(reader.read_bit().is_err());
        assert!(reader.read_bits(1).is_err());
    }
}

#[test]
fn test_fuzz_varint_decoder_arbitrary_noise() {
    let mut rng = FuzzPrng::new(0x77778888);
    let mut buffer = [0u8; 16];

    for _ in 0..10_000 {
        rng.fill_bytes(&mut buffer);
        let mut reader = BitReader::new(&buffer);

        // Varint decode must either return an integer or Err(InvalidVarint), never panic or hang
        let _ = reader.read_varint();
    }
}

#[test]
fn test_bit_writer_fuzz_bounded_writes() {
    let mut rng = FuzzPrng::new(0x55554444);
    let mut buffer = [0u8; 32];

    for _ in 0..5_000 {
        let mut writer = BitWriter::new(&mut buffer);

        for _ in 0..20 {
            let width = (rng.next_u64() % 65) as usize;
            let val = rng.next_u64();
            let _ = writer.write_bits(val, width);
        }

        // Bit offset must not exceed total buffer bits
        assert!(writer.bit_offset() <= buffer.len() * 8);
    }
}
