//! Asymmetric Numeral Systems (rANS) streaming entropy codec for MMO replication.
//!
//! Provides optimal entropy coding of quantized kinematic deltas using 32-bit streaming
//! range Asymmetric Numeral Systems (rANS) with O(1) table-driven symbol decoding.
//! Achieves sub-1.1 byte per entity wire compression with zero heap allocations during serialization.

use crate::error::NetError;

/// Lower bound of the 32-bit rANS state machine (2^16 = 65,536).
pub const RANS_LOWER_BOUND: u32 = 1 << 16;

/// Scale factor for normalized symbol probabilities (M = 2^8 = 256).
pub const RANS_SCALE_BITS: u32 = 8;
/// Total probability sum across all alphabet symbols.
pub const RANS_SCALE_SUM: usize = 1 << RANS_SCALE_BITS;

/// Pre-calculated cumulative frequency distribution table for rANS coding.
#[derive(Debug, Clone)]
pub struct RansSymbolTable {
    /// Normalized frequency for each symbol (sum must equal 256).
    pub freqs: [u16; 256],
    /// Cumulative frequency prefix sums.
    pub cum_freqs: [u16; 257],
    /// Fast O(1) inverse lookup mapping slot in 0..256 to symbol index.
    pub slot_to_symbol: [u8; RANS_SCALE_SUM],
}

impl RansSymbolTable {
    /// Constructs a normalized rANS frequency table from an array of raw symbol frequencies.
    ///
    /// The input frequencies are normalized so their sum equals exactly `RANS_SCALE_SUM` (256),
    /// ensuring every symbol with non-zero weight has at least frequency 1.
    pub fn from_frequencies(raw_freqs: &[u32]) -> Result<Self, NetError> {
        if raw_freqs.is_empty() || raw_freqs.len() > 256 {
            return Err(NetError::CorruptedData);
        }

        let mut freqs = [0u16; 256];
        let mut cum_freqs = [0u16; 257];
        let mut slot_to_symbol = [0u8; RANS_SCALE_SUM];

        let total_raw: u32 = raw_freqs.iter().sum();
        if total_raw == 0 {
            return Err(NetError::CorruptedData);
        }

        // Initial proportional scaling
        let mut current_sum = 0usize;
        for (i, &raw) in raw_freqs.iter().enumerate() {
            if raw > 0 {
                let scaled =
                    ((raw as u64 * (RANS_SCALE_SUM as u64)) / (total_raw as u64)).max(1) as u16;
                freqs[i] = scaled;
                current_sum += scaled as usize;
            }
        }

        // Adjust rounding discrepancies to guarantee sum == 256
        while current_sum > RANS_SCALE_SUM {
            // Find symbol with largest freq > 1 to decrement
            let mut max_idx = 0;
            let mut max_val = 0;
            for (i, &f) in freqs.iter().enumerate().take(raw_freqs.len()) {
                if f > max_val && f > 1 {
                    max_val = f;
                    max_idx = i;
                }
            }
            if max_val <= 1 {
                break;
            }
            freqs[max_idx] -= 1;
            current_sum -= 1;
        }

        while current_sum < RANS_SCALE_SUM {
            // Find symbol with largest freq to increment
            let mut max_idx = 0;
            let mut max_val = 0;
            for (i, &f) in freqs.iter().enumerate().take(raw_freqs.len()) {
                if f > max_val {
                    max_val = f;
                    max_idx = i;
                }
            }
            freqs[max_idx] += 1;
            current_sum += 1;
        }

        // Build cumulative frequencies
        let mut running_sum = 0u16;
        for i in 0..256 {
            cum_freqs[i] = running_sum;
            running_sum += freqs[i];
        }
        cum_freqs[256] = running_sum;

        // Build O(1) inverse lookup table
        for (sym, &f) in freqs.iter().enumerate().take(raw_freqs.len()) {
            let start = (cum_freqs[sym] as usize).min(RANS_SCALE_SUM);
            let end = (start + (f as usize)).min(RANS_SCALE_SUM);
            if start < end {
                slot_to_symbol[start..end].fill(sym as u8);
            }
        }

        Ok(Self {
            freqs,
            cum_freqs,
            slot_to_symbol,
        })
    }

    /// Constructs a uniform distribution table across `symbol_count` discrete symbols.
    pub fn uniform(symbol_count: usize) -> Self {
        let count = symbol_count.clamp(1, 256);
        let weight_per_sym = (RANS_SCALE_SUM / count) as u16;
        let remainder = RANS_SCALE_SUM % count;

        let mut freqs = [0u16; 256];
        let mut cum_freqs = [0u16; 257];
        let mut slot_to_symbol = [0u8; RANS_SCALE_SUM];

        let mut current_sum = 0u16;
        for i in 0..count {
            let f = weight_per_sym + if i < remainder { 1 } else { 0 };
            freqs[i] = f;
            cum_freqs[i] = current_sum;
            let start = current_sum as usize;
            let end = (start + f as usize).min(RANS_SCALE_SUM);
            slot_to_symbol[start..end].fill(i as u8);
            current_sum += f;
        }
        cum_freqs[count..=256].fill(current_sum);

        Self {
            freqs,
            cum_freqs,
            slot_to_symbol,
        }
    }

    /// Constructs a standard MMO kinematic delta distribution table.
    ///
    /// Calibrated for typical multiplayer kinematic distributions:
    /// - 60% stationary / resting updates
    /// - 30% small forward displacements
    /// - 8% medium turns / tactical maneuvers
    /// - 2% full keyframe refreshes
    pub fn mmo_kinematic_tier_table() -> Self {
        let raw = [600, 300, 80, 20];
        match Self::from_frequencies(&raw) {
            Ok(table) => table,
            Err(_) => Self::uniform(4),
        }
    }

    /// Constructs a heading/yaw delta distribution table.
    ///
    /// Calibrated for heading continuity:
    /// - 70% zero heading change (straight-line running)
    /// - 20% +/- 1-step minor adjustments
    /// - 10% larger rotational sweeps
    pub fn mmo_heading_delta_table() -> Self {
        let mut raw = [1u32; 16];
        raw[0] = 700; // zero delta
        raw[1] = 100; // +1 step
        raw[15] = 100; // -1 step (wrapped)
        raw[2] = 20;
        raw[14] = 20;
        match Self::from_frequencies(&raw) {
            Ok(table) => table,
            Err(_) => Self::uniform(16),
        }
    }
}

/// Streaming 32-bit rANS encoder operating in LIFO order without heap allocation.
pub struct RansEncoder {
    state: u32,
}

impl Default for RansEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl RansEncoder {
    /// Initializes a new rANS encoder with starting state L = 65,536.
    pub fn new() -> Self {
        Self {
            state: RANS_LOWER_BOUND,
        }
    }

    /// Encodes a single symbol using the given probability table.
    ///
    /// Emits renormalized bytes into `out` when the state exceeds the upper bound.
    pub fn put_symbol(&mut self, table: &RansSymbolTable, symbol: u8, out: &mut Vec<u8>) {
        let s = symbol as usize;
        let freq = table.freqs[s] as u32;
        let start = table.cum_freqs[s] as u32;

        if freq == 0 {
            return;
        }

        // Renormalize: emit bytes if state would exceed 256 * L - 1
        let max_state =
            (RANS_LOWER_BOUND / (RANS_SCALE_SUM as u32)) * freq * (RANS_SCALE_SUM as u32);
        while self.state >= max_state {
            out.push((self.state & 0xFF) as u8);
            self.state >>= 8;
        }

        // State transition: x' = floor(x / freq) * M + (x % freq) + start
        let q = self.state / freq;
        let r = self.state % freq;
        self.state = (q << RANS_SCALE_BITS) + r + start;
    }

    /// Flushes the final 4-byte state into `out` and returns total bytes emitted.
    pub fn flush(self, out: &mut Vec<u8>) -> usize {
        out.extend_from_slice(&self.state.to_le_bytes());
        out.len()
    }

    /// Encodes an entire slice of symbols in reverse order so they can be decoded forward.
    pub fn encode_symbols_forward(
        table: &RansSymbolTable,
        symbols: &[u8],
        out: &mut Vec<u8>,
    ) -> usize {
        let mut encoder = Self::new();
        // rANS is a stack (LIFO): encode backward to decode forward
        for &sym in symbols.iter().rev() {
            encoder.put_symbol(table, sym, out);
        }
        encoder.flush(out)
    }
}

/// Streaming 32-bit rANS decoder operating in forward order without heap allocation.
pub struct RansDecoder<'a> {
    state: u32,
    stream: &'a [u8],
    cursor: usize,
}

impl<'a> RansDecoder<'a> {
    /// Initializes a decoder from a flushed rANS byte stream.
    ///
    /// Reads the initial 4-byte state from the end of the stream.
    pub fn new(data: &'a [u8]) -> Result<Self, NetError> {
        if data.len() < 4 {
            return Err(NetError::TruncatedPacket {
                expected_len: 4,
                actual_len: data.len(),
            });
        }

        // The 4-byte state was written at the end of the byte stream
        let stream_len = data.len() - 4;
        let mut state_bytes = [0u8; 4];
        if let Some(tail) = data.get(stream_len..stream_len + 4) {
            state_bytes.copy_from_slice(tail);
        }
        let state = u32::from_le_bytes(state_bytes);

        Ok(Self {
            state,
            stream: &data[..stream_len],
            cursor: stream_len,
        })
    }

    /// Decodes a single symbol using O(1) table lookup and advances the state.
    #[inline]
    pub fn get_symbol(&mut self, table: &RansSymbolTable) -> u8 {
        // Fast O(1) symbol lookup from current slot
        let slot = (self.state & ((RANS_SCALE_SUM as u32) - 1)) as usize;
        let sym = table.slot_to_symbol[slot];

        let s = sym as usize;
        let freq = table.freqs[s] as u32;
        let start = table.cum_freqs[s] as u32;

        // Inverse state transition: x' = freq * (x >> 8) + (slot - start)
        let offset = (slot as u32).saturating_sub(start);
        self.state = freq
            .saturating_mul(self.state >> RANS_SCALE_BITS)
            .saturating_add(offset);

        // Renormalize: consume bytes from stream while state < L
        while self.state < RANS_LOWER_BOUND && self.cursor > 0 {
            self.cursor -= 1;
            let byte = self.stream.get(self.cursor).copied().unwrap_or(0) as u32;
            self.state = (self.state << 8) | byte;
        }

        sym
    }

    /// Decodes `count` symbols in forward order directly into `out`.
    pub fn decode_symbols(
        table: &RansSymbolTable,
        data: &'a [u8],
        out: &mut [u8],
    ) -> Result<usize, NetError> {
        let mut decoder = Self::new(data)?;
        for sym in out.iter_mut() {
            *sym = decoder.get_symbol(table);
        }
        Ok(out.len())
    }
}

/// Encodes an array of delta kinematic entities into a highly compressed rANS bitstream.
///
/// Combines tier selection and magnitude offsets into an entropy-compressed stream.
pub fn compress_kinematic_stream(
    tier_table: &RansSymbolTable,
    heading_table: &RansSymbolTable,
    tier_symbols: &[u8],
    heading_symbols: &[u8],
    out: &mut Vec<u8>,
) -> usize {
    assert_eq!(tier_symbols.len(), heading_symbols.len());
    let mut encoder = RansEncoder::new();

    // Interleave symbols in reverse order
    for i in (0..tier_symbols.len()).rev() {
        encoder.put_symbol(heading_table, heading_symbols[i], out);
        encoder.put_symbol(tier_table, tier_symbols[i], out);
    }

    encoder.flush(out)
}

/// Decompresses an interleaved kinematic rANS stream into forward symbol arrays.
pub fn decompress_kinematic_stream(
    tier_table: &RansSymbolTable,
    heading_table: &RansSymbolTable,
    data: &[u8],
    out_tiers: &mut [u8],
    out_headings: &mut [u8],
) -> Result<(), NetError> {
    assert_eq!(out_tiers.len(), out_headings.len());
    let mut decoder = RansDecoder::new(data)?;

    for i in 0..out_tiers.len() {
        out_tiers[i] = decoder.get_symbol(tier_table);
        out_headings[i] = decoder.get_symbol(heading_table);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rans_symbol_table_normalization_and_sum() {
        let table = RansSymbolTable::mmo_kinematic_tier_table();
        let sum: u16 = table.freqs.iter().sum();
        assert_eq!(sum as usize, RANS_SCALE_SUM);
        assert_eq!(table.cum_freqs[256] as usize, RANS_SCALE_SUM);

        // Verify slot lookup matches cumulative ranges
        for slot in 0..RANS_SCALE_SUM {
            let sym = table.slot_to_symbol[slot] as usize;
            let start = table.cum_freqs[sym] as usize;
            let end = start + (table.freqs[sym] as usize);
            assert!(slot >= start && slot < end);
        }
    }

    #[test]
    fn test_rans_encode_decode_roundtrip_identity() {
        let table = RansSymbolTable::mmo_kinematic_tier_table();

        // 1,000 synthetic symbols with typical skewed MMO distribution
        let mut original = Vec::new();
        for i in 0..1000 {
            let sym = match i % 10 {
                0..=5 => 0, // 60% stationary
                6..=8 => 1, // 30% small
                9 => 2,     // 10% medium
                _ => 3,
            };
            original.push(sym);
        }

        let mut encoded_bytes = Vec::new();
        RansEncoder::encode_symbols_forward(&table, &original, &mut encoded_bytes);

        let mut decoded = vec![0u8; original.len()];
        RansDecoder::decode_symbols(&table, &encoded_bytes, &mut decoded).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn test_interleaved_kinematic_stream_compression_ratio() {
        let tier_table = RansSymbolTable::mmo_kinematic_tier_table();
        let heading_table = RansSymbolTable::mmo_heading_delta_table();

        const NUM_ENTITIES: usize = 1000;
        let mut tiers = Vec::with_capacity(NUM_ENTITIES);
        let mut headings = Vec::with_capacity(NUM_ENTITIES);

        for i in 0..NUM_ENTITIES {
            let t = match i % 10 {
                0..=5 => 0, // Stationary
                6..=8 => 1, // Small delta
                _ => 2,     // Medium delta
            };
            let h = match i % 10 {
                0..=6 => 0, // Straight heading
                7 | 8 => 1, // Minor turn
                _ => 15,
            };
            tiers.push(t);
            headings.push(h);
        }

        let mut compressed = Vec::new();
        let total_bytes = compress_kinematic_stream(
            &tier_table,
            &heading_table,
            &tiers,
            &headings,
            &mut compressed,
        );

        println!(
            "rANS Interleaved Compression: {} entities compressed into {} bytes ({:.2} B/entity)",
            NUM_ENTITIES,
            total_bytes,
            (total_bytes as f64) / (NUM_ENTITIES as f64)
        );

        // Sub-1.1 B/entity requirement: 1,000 entities in < 1,100 bytes
        assert!(
            total_bytes < 1100,
            "Total compressed bytes ({}) must be < 1,100 bytes ({:.2} B/entity)",
            total_bytes,
            (total_bytes as f64) / (NUM_ENTITIES as f64)
        );

        // Verify lossless decompression
        let mut dec_tiers = vec![0u8; NUM_ENTITIES];
        let mut dec_headings = vec![0u8; NUM_ENTITIES];
        decompress_kinematic_stream(
            &tier_table,
            &heading_table,
            &compressed,
            &mut dec_tiers,
            &mut dec_headings,
        )
        .unwrap();

        assert_eq!(tiers, dec_tiers);
        assert_eq!(headings, dec_headings);
    }
}
