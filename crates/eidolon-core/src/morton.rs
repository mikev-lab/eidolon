//! 64-bit Morton code (Z-order space-filling curve) 3D spatial indexing primitives.
//!
//! Provides deterministic 3D-to-1D spatial clustering without external dependencies,
//! mapping contiguous spatial volumes into contiguous memory addresses to optimize
//! CPU L1/L2 cache prefetching during high-density AoI queries.

/// Coordinate bias ensuring negative integer coordinates map to positive unsigned values.
pub const MORTON_COORDINATE_BIAS: i32 = 1_048_576; // 2^20

/// Maximum representable discrete coordinate value after biasing (21 bits: [0, 2^21 - 1]).
pub const MORTON_MAX_COORDINATE: u32 = 2_097_151;

/// Precomputed compile-time table spreading an 8-bit integer into 24 bits (every 3rd bit).
const fn make_spread_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut val = 0u32;
        let mut bit = 0;
        while bit < 8 {
            if (i & (1 << bit)) != 0 {
                val |= 1 << (bit * 3);
            }
            bit += 1;
        }
        table[i] = val;
        i += 1;
    }
    table
}

/// Static spread lookup table evaluated at compile time.
pub static SPREAD_TABLE: [u32; 256] = make_spread_table();

/// Spreads the lower 21 bits of an integer into every 3rd bit position (bits 0, 3, 6, 9, ...).
#[inline]
pub fn spread_bits_21(x: u32) -> u64 {
    let clamped = x & MORTON_MAX_COORDINATE;
    let b0 = (clamped & 0xFF) as usize;
    let b1 = ((clamped >> 8) & 0xFF) as usize;
    let b2 = ((clamped >> 16) & 0x1F) as usize;

    (SPREAD_TABLE[b0] as u64)
        | ((SPREAD_TABLE[b1] as u64) << 24)
        | ((SPREAD_TABLE[b2] as u64) << 48)
}

/// Compacts an 8-bit integer from a 24-bit word where bits are located at positions 0, 3, 6, 9, 12, 15, 18, 21.
#[inline]
pub fn compact_spread_24(w: u32) -> u8 {
    let b0 = (w & 1) as u8;
    let b1 = ((w >> 3) & 1) as u8;
    let b2 = ((w >> 6) & 1) as u8;
    let b3 = ((w >> 9) & 1) as u8;
    let b4 = ((w >> 12) & 1) as u8;
    let b5 = ((w >> 15) & 1) as u8;
    let b6 = ((w >> 18) & 1) as u8;
    let b7 = ((w >> 21) & 1) as u8;
    b0 | (b1 << 1) | (b2 << 2) | (b3 << 3) | (b4 << 4) | (b5 << 5) | (b6 << 6) | (b7 << 7)
}

/// Compacts every 3rd bit of a 64-bit integer back into a 21-bit integer.
#[inline]
pub fn compact_bits_21(w: u64) -> u32 {
    let c0 = compact_spread_24(w as u32) as u32;
    let c1 = compact_spread_24((w >> 24) as u32) as u32;
    let c2 = compact_spread_24((w >> 48) as u32) as u32;
    c0 | (c1 << 8) | (c2 << 16)
}

/// Encodes biased discrete 3D coordinates (x, y, z in [0, 2^21 - 1]) into a 64-bit Morton code.
///
/// Bit layout:
/// - X coordinate bits placed at positions 0, 3, 6, 9, ...
/// - Y coordinate bits placed at positions 1, 4, 7, 10, ...
/// - Z coordinate bits placed at positions 2, 5, 8, 11, ...
#[inline]
pub fn morton_encode_u32(x: u32, y: u32, z: u32) -> u64 {
    spread_bits_21(x) | (spread_bits_21(y) << 1) | (spread_bits_21(z) << 2)
}

/// Decodes a 64-bit Morton code back into biased discrete 3D coordinates (x, y, z).
#[inline]
pub fn morton_decode_u32(code: u64) -> (u32, u32, u32) {
    let x = compact_bits_21(code);
    let y = compact_bits_21(code >> 1);
    let z = compact_bits_21(code >> 2);
    (x, y, z)
}

/// Encodes signed discrete 3D coordinates (e.g. spatial grid cell indices) into a 64-bit Morton code.
///
/// Coordinates are biased by `MORTON_COORDINATE_BIAS` to support negative values
/// in the range `[-1,000,000, +1,000,000]`.
#[inline]
pub fn morton_encode_i32(x: i32, y: i32, z: i32) -> u64 {
    let bx =
        (x.saturating_add(MORTON_COORDINATE_BIAS)).clamp(0, MORTON_MAX_COORDINATE as i32) as u32;
    let by =
        (y.saturating_add(MORTON_COORDINATE_BIAS)).clamp(0, MORTON_MAX_COORDINATE as i32) as u32;
    let bz =
        (z.saturating_add(MORTON_COORDINATE_BIAS)).clamp(0, MORTON_MAX_COORDINATE as i32) as u32;
    morton_encode_u32(bx, by, bz)
}

/// Decodes a 64-bit Morton code back into signed discrete 3D coordinates (x, y, z).
#[inline]
pub fn morton_decode_i32(code: u64) -> (i32, i32, i32) {
    let (bx, by, bz) = morton_decode_u32(code);
    (
        (bx as i32) - MORTON_COORDINATE_BIAS,
        (by as i32) - MORTON_COORDINATE_BIAS,
        (bz as i32) - MORTON_COORDINATE_BIAS,
    )
}

/// Encodes a continuous fixed-point 3D position into a 64-bit Morton code.
#[inline]
pub fn morton_encode_vec3(pos: crate::fixed::Vec3Fix) -> u64 {
    morton_encode_i32(pos.x.to_i32(), pos.y.to_i32(), pos.z.to_i32())
}

/// Decodes a 64-bit Morton code back into a continuous fixed-point 3D position.
#[inline]
pub fn morton_decode_vec3(code: u64) -> crate::fixed::Vec3Fix {
    let (x, y, z) = morton_decode_i32(code);
    crate::fixed::Vec3Fix::new(
        crate::fixed::Fixed64::from_i32(x),
        crate::fixed::Fixed64::from_i32(y),
        crate::fixed::Fixed64::from_i32(z),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spread_and_compact_invertibility() {
        let test_values = [
            0,
            1,
            2,
            7,
            63,
            1024,
            65535,
            1_000_000,
            MORTON_MAX_COORDINATE,
        ];
        for &val in &test_values {
            let spread = spread_bits_21(val);
            let compacted = compact_bits_21(spread);
            assert_eq!(compacted, val, "Failed round-trip for value {}", val);
        }
    }

    #[test]
    fn test_morton_encode_decode_u32_roundtrip() {
        let cases = [
            (0, 0, 0),
            (1, 2, 3),
            (100, 200, 300),
            (65535, 32768, 16384),
            (
                MORTON_MAX_COORDINATE,
                MORTON_MAX_COORDINATE,
                MORTON_MAX_COORDINATE,
            ),
        ];

        for (x, y, z) in cases {
            let code = morton_encode_u32(x, y, z);
            let (dx, dy, dz) = morton_decode_u32(code);
            assert_eq!((dx, dy, dz), (x, y, z), "Failed for ({}, {}, {})", x, y, z);
        }
    }

    #[test]
    fn test_morton_encode_decode_i32_roundtrip() {
        let cases = [
            (0, 0, 0),
            (-1, -1, -1),
            (-500, 1200, -3400),
            (-500_000, 250_000, -100_000),
            (500_000, -500_000, 123_456),
        ];

        for (x, y, z) in cases {
            let code = morton_encode_i32(x, y, z);
            let (dx, dy, dz) = morton_decode_i32(code);
            assert_eq!((dx, dy, dz), (x, y, z), "Failed for ({}, {}, {})", x, y, z);
        }
    }

    #[test]
    fn test_morton_spatial_locality() {
        let p1 = morton_encode_i32(10, 10, 10);
        let p2 = morton_encode_i32(10, 10, 11);
        let p_far = morton_encode_i32(1000, 1000, 1000);

        assert!(p1 < p2);
        assert!(p2 < p_far);
    }
}
