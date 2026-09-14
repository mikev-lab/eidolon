//! Asymmetric spatial coordinate and discrete heading quantization.
//!
//! Maps continuous spatial vectors into bitpacked bounded integer representations,
//! enabling sub-millimeter precision within a compact wire footprint.

use core::f64::consts::PI;

use crate::fixed::{Fixed64, Vec3Fix};

/// Horizontal cell size in meters (64.0m).
pub const CELL_HORIZONTAL_SIZE: Fixed64 = Fixed64::from_i32(64);

/// Vertical cell size in meters (32.0m).
pub const CELL_VERTICAL_SIZE: Fixed64 = Fixed64::from_i32(32);

/// Maximum 16-bit integer representation for horizontal coordinates (65,535).
pub const MAX_QUANTIZED_HORIZONTAL: u16 = u16::MAX;

/// Maximum 12-bit integer representation for vertical elevation (4,095).
pub const MAX_QUANTIZED_VERTICAL: u16 = 0x0FFF;

/// Horizontal resolution in meters: 64.0 / 65535.0 approx 0.0009765m (0.976 mm).
pub const HORIZONTAL_RESOLUTION_METERS: f64 = 64.0 / (MAX_QUANTIZED_HORIZONTAL as f64);

/// Vertical resolution in meters: 32.0 / 4095.0 approx 0.0078144m (7.81 mm).
pub const VERTICAL_RESOLUTION_METERS: f64 = 32.0 / (MAX_QUANTIZED_VERTICAL as f64);

/// Transform flag bit (bit 3 of flags nibble / bit 7 of byte 6) indicating a 17-byte Cell Anchor update.
///
/// When set, the wire representation prepends 6-byte cell coordinates `(cx: i16, cy: i16, cz: i16)`
/// ahead of the 7-byte transform, allowing observers to reconstruct global positions upon AoI entry or cell crossing.
pub const FLAG_CELL_ANCHOR: u8 = 1 << 3;

/// Quantized local cell-relative coordinates.
///
/// Encodes horizontal positions X and Z as 16-bit unsigned integers
/// and vertical elevation Y as a 12-bit unsigned integer (bounded to 0..=4095).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct QuantizedCellCoord {
    /// Quantized X offset within the local cell (0..=65535).
    pub x: u16,
    /// Quantized vertical elevation within the vertical cell band (0..=4095).
    pub y: u16,
    /// Quantized Z offset within the local cell (0..=65535).
    pub z: u16,
}

impl QuantizedCellCoord {
    /// Constructs a quantized cell coordinate from raw integers with bounds checking.
    #[inline]
    pub const fn new(x: u16, y: u16, z: u16) -> Self {
        let clamped_y = if y > MAX_QUANTIZED_VERTICAL {
            MAX_QUANTIZED_VERTICAL
        } else {
            y
        };
        Self { x, y: clamped_y, z }
    }

    /// Quantizes local cell continuous coordinates (0..64m horizontal, 0..32m vertical).
    pub fn quantize(local_x: Fixed64, local_y: Fixed64, local_z: Fixed64) -> Self {
        // Clamp to positive cell bounds branchlessly
        let x_clamped = local_x.clamp(Fixed64::ZERO, CELL_HORIZONTAL_SIZE);
        let y_clamped = local_y.clamp(Fixed64::ZERO, CELL_VERTICAL_SIZE);
        let z_clamped = local_z.clamp(Fixed64::ZERO, CELL_HORIZONTAL_SIZE);

        // Fast bitshift quantization: 64m cell = 2^6, 32m cell = 2^5
        let x_scaled = (x_clamped.raw() as u64) >> 6;
        let q_x = ((x_scaled * (MAX_QUANTIZED_HORIZONTAL as u64)) >> 32)
            .min(MAX_QUANTIZED_HORIZONTAL as u64) as u16;

        let y_scaled = (y_clamped.raw() as u64) >> 5;
        let q_y = ((y_scaled * (MAX_QUANTIZED_VERTICAL as u64)) >> 32)
            .min(MAX_QUANTIZED_VERTICAL as u64) as u16;

        let z_scaled = (z_clamped.raw() as u64) >> 6;
        let q_z = ((z_scaled * (MAX_QUANTIZED_HORIZONTAL as u64)) >> 32)
            .min(MAX_QUANTIZED_HORIZONTAL as u64) as u16;

        Self {
            x: q_x,
            y: q_y,
            z: q_z,
        }
    }

    /// Quantizes f64 coordinates.
    pub fn from_f64(local_x: f64, local_y: f64, local_z: f64) -> Self {
        let q_x =
            (local_x.clamp(0.0, 64.0) / 64.0 * (MAX_QUANTIZED_HORIZONTAL as f64)).round() as u32;
        let q_y =
            (local_y.clamp(0.0, 32.0) / 32.0 * (MAX_QUANTIZED_VERTICAL as f64)).round() as u32;
        let q_z =
            (local_z.clamp(0.0, 64.0) / 64.0 * (MAX_QUANTIZED_HORIZONTAL as f64)).round() as u32;

        Self {
            x: q_x.min(MAX_QUANTIZED_HORIZONTAL as u32) as u16,
            y: q_y.min(MAX_QUANTIZED_VERTICAL as u32) as u16,
            z: q_z.min(MAX_QUANTIZED_HORIZONTAL as u32) as u16,
        }
    }

    /// Dequantizes the integer coordinate back to continuous fixed-point position.
    pub fn dequantize(self) -> Vec3Fix {
        let x_frac =
            Fixed64::from_i32(self.x as i32) / Fixed64::from_i32(MAX_QUANTIZED_HORIZONTAL as i32);
        let y_frac = Fixed64::from_i32((self.y & MAX_QUANTIZED_VERTICAL) as i32)
            / Fixed64::from_i32(MAX_QUANTIZED_VERTICAL as i32);
        let z_frac =
            Fixed64::from_i32(self.z as i32) / Fixed64::from_i32(MAX_QUANTIZED_HORIZONTAL as i32);

        Vec3Fix {
            x: x_frac * CELL_HORIZONTAL_SIZE,
            y: y_frac * CELL_VERTICAL_SIZE,
            z: z_frac * CELL_HORIZONTAL_SIZE,
        }
    }

    /// Quantizes a continuous global world position into its enclosing grid cell
    /// indices and normalized local-cell 16-bit quantized offset.
    ///
    /// Works across arbitrary positive and negative continuous coordinates using
    /// bit-exact Euclidean cell decomposition:
    /// - Horizontal cell size: 64.0m (2^6 m, 38-bit raw in 32.32 representation)
    /// - Vertical cell size: 32.0m (2^5 m, 37-bit raw in 32.32 representation)
    ///
    /// # World Bounds and Fixed64 Invariant:
    /// When transmitting discrete cell indices as 16-bit signed integers `(i16, i16, i16)`
    /// over the wire, horizontal indices span `[-32,768, 32,767] * 64m = [-2,097,152m, +2,097,151m]`
    /// (a 4,194 km x 4,194 km world map), which safely fits within the `Fixed64` 32.32 range
    /// of `[-2^31, 2^31 - 1]` meters (approx +/- 2.14 billion meters) without arithmetic overflow.
    #[inline]
    pub fn quantize_from_global(pos: Vec3Fix) -> (i32, i32, i32, Self) {
        let cell_x = (pos.x.raw() >> 38) as i32;
        let cell_y = (pos.y.raw() >> 37) as i32;
        let cell_z = (pos.z.raw() >> 38) as i32;

        let local_x = pos.x - Fixed64::from_i32(cell_x * 64);
        let local_y = pos.y - Fixed64::from_i32(cell_y * 32);
        let local_z = pos.z - Fixed64::from_i32(cell_z * 64);

        let quant = Self::quantize(local_x, local_y, local_z);
        (cell_x, cell_y, cell_z, quant)
    }

    /// Reconstructs a continuous global world position from discrete cell indices
    /// and a quantized local-cell coordinate.
    #[inline]
    pub fn dequantize_to_global(cell_x: i32, cell_y: i32, cell_z: i32, quant: Self) -> Vec3Fix {
        let cell_origin = Vec3Fix {
            x: Fixed64::from_i32(cell_x * 64),
            y: Fixed64::from_i32(cell_y * 32),
            z: Fixed64::from_i32(cell_z * 64),
        };
        cell_origin + quant.dequantize()
    }

    /// Dequantizes the coordinate to continuous f64 coordinates.
    pub fn to_f64(self) -> (f64, f64, f64) {
        let x = (self.x as f64) * HORIZONTAL_RESOLUTION_METERS;
        let y = ((self.y & MAX_QUANTIZED_VERTICAL) as f64) * VERTICAL_RESOLUTION_METERS;
        let z = (self.z as f64) * HORIZONTAL_RESOLUTION_METERS;
        (x, y, z)
    }

    /// Validates that elevation conforms to the 12-bit vertical limit.
    #[inline]
    pub const fn is_valid(self) -> bool {
        self.y <= MAX_QUANTIZED_VERTICAL
    }

    /// Bitpacks coordinates (44 bits), discrete yaw (8 bits), and movement flags (4 bits)
    /// into an exact 7-byte (56-bit) wire buffer.
    ///
    /// Bit layout:
    /// - Bytes 0..1: X coordinate (16 bits)
    /// - Bytes 2..3: Z coordinate (16 bits)
    /// - Byte 4: Y coordinate low 8 bits (bits 0..7)
    /// - Byte 5: Y coordinate high 4 bits (bits 0..3) | Yaw low 4 bits (bits 4..7)
    /// - Byte 6: Yaw high 4 bits (bits 0..3) | Movement flags (bits 4..7)
    pub fn pack_with_yaw_and_flags(self, yaw: QuantizedYaw, flags: u8) -> [u8; 7] {
        let y_bounded = self.y & MAX_QUANTIZED_VERTICAL;
        let yaw_val = yaw.as_byte();
        let flags_nibble = flags & 0x0F;

        [
            (self.x & 0xFF) as u8,
            ((self.x >> 8) & 0xFF) as u8,
            (self.z & 0xFF) as u8,
            ((self.z >> 8) & 0xFF) as u8,
            (y_bounded & 0xFF) as u8,
            (((y_bounded >> 8) & 0x0F) as u8) | ((yaw_val & 0x0F) << 4),
            ((yaw_val >> 4) & 0x0F) | (flags_nibble << 4),
        ]
    }

    /// Unpacks coordinates, yaw, and movement flags from a 7-byte wire buffer.
    pub fn unpack_with_yaw_and_flags(bytes: [u8; 7]) -> (Self, QuantizedYaw, u8) {
        let x = (bytes[0] as u16) | ((bytes[1] as u16) << 8);
        let z = (bytes[2] as u16) | ((bytes[3] as u16) << 8);
        let y = (bytes[4] as u16) | (((bytes[5] & 0x0F) as u16) << 8);
        let yaw = ((bytes[5] >> 4) & 0x0F) | ((bytes[6] & 0x0F) << 4);
        let flags = (bytes[6] >> 4) & 0x0F;

        (Self { x, y, z }, QuantizedYaw::from_byte(yaw), flags)
    }
}

/// Quantized discrete yaw heading.
///
/// Encodes continuous 360-degree heading into a single unsigned byte (256 discrete angles),
/// achieving approx 1.406-degree resolution with branchless shortest-arc delta arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct QuantizedYaw(pub u8);

impl QuantizedYaw {
    /// Discrete angular step size in degrees (360.0 / 256 = 1.40625 degrees).
    pub const STEP_DEGREES: f64 = 360.0 / 256.0;

    /// Discrete angular step size in radians (2*PI / 256 = PI / 128 radians).
    pub const STEP_RADIANS: f64 = (2.0 * PI) / 256.0;

    /// Facing North / 0 degrees.
    pub const NORTH: Self = Self(0);

    /// Facing East / 90 degrees.
    pub const EAST: Self = Self(64);

    /// Facing South / 180 degrees.
    pub const SOUTH: Self = Self(128);

    /// Facing West / 270 degrees.
    pub const WEST: Self = Self(192);

    /// Constructs quantized yaw from a raw byte.
    #[inline]
    pub const fn from_byte(val: u8) -> Self {
        Self(val)
    }

    /// Returns the underlying raw discrete angle byte.
    #[inline]
    pub const fn as_byte(self) -> u8 {
        self.0
    }

    /// Constructs quantized yaw from radians [0, 2*PI).
    pub fn from_radians(rad: f64) -> Self {
        let two_pi = 2.0 * PI;
        let norm = rad.rem_euclid(two_pi);
        let discrete = (norm / two_pi * 256.0).round() as u32;
        Self((discrete % 256) as u8)
    }

    /// Converts quantized yaw to radians [0, 2*PI).
    pub fn to_radians(self) -> f64 {
        (self.0 as f64) * Self::STEP_RADIANS
    }

    /// Constructs quantized yaw from degrees [0, 360).
    pub fn from_degrees(deg: f64) -> Self {
        let norm = deg.rem_euclid(360.0);
        let discrete = (norm / 360.0 * 256.0).round() as u32;
        Self((discrete % 256) as u8)
    }

    /// Converts quantized yaw to degrees [0, 360).
    pub fn to_degrees(self) -> f64 {
        (self.0 as f64) * Self::STEP_DEGREES
    }

    /// Computes the signed shortest-arc angular delta from self to target.
    ///
    /// Returns a signed 8-bit integer (-128..=127 discrete steps):
    /// - Positive values indicate counter-clockwise turning.
    /// - Negative values indicate clockwise turning.
    ///
    /// Seamlessly handles the 255 to 0 discrete boundary rollover in a single branchless instruction.
    #[inline]
    pub const fn shortest_arc_delta(self, target: Self) -> i8 {
        target.0.wrapping_sub(self.0) as i8
    }

    /// Advances self toward target along the shortest arc by at most `max_step` discrete steps.
    pub fn advance_toward(self, target: Self, max_step: u8) -> Self {
        let delta = self.shortest_arc_delta(target);
        if delta == 0 {
            return self;
        }

        if delta > 0 {
            let step = (delta as u8).min(max_step);
            Self(self.0.wrapping_add(step))
        } else {
            let step = delta.unsigned_abs().min(max_step);
            Self(self.0.wrapping_sub(step))
        }
    }
}
