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

/// 6-bit discrete yaw heading (64 discrete angles, approx 5.625 degree resolution).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct QuantizedYaw6Bit(pub u8);

impl QuantizedYaw6Bit {
    /// Discrete angular step size in degrees (360.0 / 64 = 5.625 degrees).
    pub const STEP_DEGREES: f64 = 360.0 / 64.0;

    /// Constructs 6-bit quantized yaw from degrees [0, 360).
    pub fn from_degrees(deg: f64) -> Self {
        let norm = deg.rem_euclid(360.0);
        let discrete = (norm / 360.0 * 64.0).round() as u32;
        Self((discrete % 64) as u8)
    }

    /// Converts 6-bit quantized yaw to degrees.
    pub fn to_degrees(self) -> f64 {
        (self.0 as f64) * Self::STEP_DEGREES
    }

    /// Downsamples standard 8-bit QuantizedYaw to 6-bit resolution.
    #[inline]
    pub const fn from_quantized_yaw(yaw: QuantizedYaw) -> Self {
        Self(yaw.as_byte() >> 2)
    }

    /// Upsamples 6-bit resolution to standard 8-bit QuantizedYaw.
    #[inline]
    pub const fn to_quantized_yaw(self) -> QuantizedYaw {
        QuantizedYaw((self.0 & 0x3F) << 2)
    }
}

/// 4-bit discrete heading (16 discrete angles, approx 22.5 degree resolution).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct QuantizedYaw4Bit(pub u8);

impl QuantizedYaw4Bit {
    /// Discrete angular step size in degrees (360.0 / 16 = 22.5 degrees).
    pub const STEP_DEGREES: f64 = 360.0 / 16.0;

    /// Constructs 4-bit quantized yaw from degrees [0, 360).
    pub fn from_degrees(deg: f64) -> Self {
        let norm = deg.rem_euclid(360.0);
        let discrete = (norm / 360.0 * 16.0).round() as u32;
        Self((discrete % 16) as u8)
    }

    /// Converts 4-bit quantized yaw to degrees.
    pub fn to_degrees(self) -> f64 {
        (self.0 as f64) * Self::STEP_DEGREES
    }

    /// Downsamples standard 8-bit QuantizedYaw to 4-bit resolution.
    #[inline]
    pub const fn from_quantized_yaw(yaw: QuantizedYaw) -> Self {
        Self(yaw.as_byte() >> 4)
    }

    /// Upsamples 4-bit resolution to standard 8-bit QuantizedYaw.
    #[inline]
    pub const fn to_quantized_yaw(self) -> QuantizedYaw {
        QuantizedYaw((self.0 & 0x0F) << 4)
    }
}

/// Midfield tier cell-relative quantized coordinates (10-bit X, 8-bit Y, 10-bit Z).
///
/// Encodes horizontal coordinates at 6.25 cm precision and vertical elevation at 12.5 cm precision.
/// Bitpacked with 6-bit yaw and 4-bit flags into an exact 5-byte buffer (28.6% bandwidth reduction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct MidfieldQuantizedCoord {
    /// 10-bit quantized X offset within cell (0..=1023).
    pub x: u16,
    /// 8-bit quantized Y elevation within vertical band (0..=255).
    pub y: u8,
    /// 10-bit quantized Z offset within cell (0..=1023).
    pub z: u16,
}

impl MidfieldQuantizedCoord {
    /// Maximum 10-bit horizontal representation.
    pub const MAX_HORIZONTAL: u16 = 1023;
    /// Maximum 8-bit vertical representation.
    pub const MAX_VERTICAL: u8 = 255;
    /// Horizontal resolution in meters: 64.0 / 1023.0 approx 0.06256m (6.25 cm).
    pub const HORIZONTAL_RESOLUTION_METERS: f64 = 64.0 / 1023.0;
    /// Vertical resolution in meters: 32.0 / 255.0 approx 0.1255m (12.5 cm).
    pub const VERTICAL_RESOLUTION_METERS: f64 = 32.0 / 255.0;

    /// Constructs a midfield coordinate from continuous coordinates.
    pub fn from_f64(local_x: f64, local_y: f64, local_z: f64) -> Self {
        let q_x = (local_x.clamp(0.0, 64.0) / 64.0 * 1023.0).round() as u32;
        let q_y = (local_y.clamp(0.0, 32.0) / 32.0 * 255.0).round() as u32;
        let q_z = (local_z.clamp(0.0, 64.0) / 64.0 * 1023.0).round() as u32;
        Self {
            x: q_x.min(1023) as u16,
            y: q_y.min(255) as u8,
            z: q_z.min(1023) as u16,
        }
    }

    /// Constructs a midfield coordinate from fixed-point coordinates.
    pub fn quantize(local_x: Fixed64, local_y: Fixed64, local_z: Fixed64) -> Self {
        Self::from_f64(local_x.to_f64(), local_y.to_f64(), local_z.to_f64())
    }

    /// Dequantizes to continuous f64 coordinates (x, y, z).
    pub fn to_f64(self) -> (f64, f64, f64) {
        (
            (self.x as f64) * Self::HORIZONTAL_RESOLUTION_METERS,
            (self.y as f64) * Self::VERTICAL_RESOLUTION_METERS,
            (self.z as f64) * Self::HORIZONTAL_RESOLUTION_METERS,
        )
    }

    /// Dequantizes to fixed-point Vec3Fix.
    pub fn dequantize(self) -> Vec3Fix {
        let (x, y, z) = self.to_f64();
        Vec3Fix::from_f64(x, y, z)
    }

    /// Packs midfield coordinate, 6-bit yaw, and 4-bit flags into an exact 5-byte wire buffer.
    pub fn pack(self, yaw: QuantizedYaw6Bit, flags: u8) -> [u8; 5] {
        let x_val = self.x & 0x03FF;
        let z_val = self.z & 0x03FF;
        let y_val = self.y;
        let yaw_val = yaw.0 & 0x3F;
        let flags_nibble = flags & 0x0F;

        [
            (x_val & 0xFF) as u8,
            (((x_val >> 8) & 0x03) as u8) | (((z_val & 0x3F) as u8) << 2),
            (((z_val >> 6) & 0x0F) as u8) | ((y_val & 0x0F) << 4),
            ((y_val >> 4) & 0x0F) | ((yaw_val & 0x0F) << 4),
            ((yaw_val >> 4) & 0x03) | (flags_nibble << 2),
        ]
    }

    /// Unpacks midfield coordinate, 6-bit yaw, and 4-bit flags from a 5-byte buffer.
    pub fn unpack(bytes: [u8; 5]) -> (Self, QuantizedYaw6Bit, u8) {
        let x = (bytes[0] as u16) | (((bytes[1] & 0x03) as u16) << 8);
        let z = (((bytes[1] >> 2) & 0x3F) as u16) | (((bytes[2] & 0x0F) as u16) << 6);
        let y = ((bytes[2] >> 4) & 0x0F) | ((bytes[3] & 0x0F) << 4);
        let yaw = ((bytes[3] >> 4) & 0x0F) | ((bytes[4] & 0x03) << 4);
        let flags = (bytes[4] >> 2) & 0x0F;

        (Self { x, y, z }, QuantizedYaw6Bit(yaw), flags)
    }
}

/// Horizon tier cell-relative quantized coordinates (6-bit X, 5-bit Y, 6-bit Z).
///
/// Encodes horizontal coordinates at 1.0 m precision and vertical elevation at 1.0 m precision.
/// Bitpacked with 4-bit heading and 3-bit flags into an exact 3-byte buffer (57.1% bandwidth reduction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct HorizonQuantizedCoord {
    /// 6-bit quantized X offset within cell (0..=63).
    pub x: u8,
    /// 5-bit quantized Y elevation within vertical band (0..=31).
    pub y: u8,
    /// 6-bit quantized Z offset within cell (0..=63).
    pub z: u8,
}

impl HorizonQuantizedCoord {
    /// Maximum 6-bit horizontal representation.
    pub const MAX_HORIZONTAL: u8 = 63;
    /// Maximum 5-bit vertical representation.
    pub const MAX_VERTICAL: u8 = 31;
    /// Horizontal resolution in meters: 64.0 / 63.0 approx 1.016m (approx 1.0 m).
    pub const HORIZONTAL_RESOLUTION_METERS: f64 = 64.0 / 63.0;
    /// Vertical resolution in meters: 32.0 / 31.0 approx 1.032m (approx 1.0 m).
    pub const VERTICAL_RESOLUTION_METERS: f64 = 32.0 / 31.0;

    /// Constructs a horizon coordinate from continuous coordinates.
    pub fn from_f64(local_x: f64, local_y: f64, local_z: f64) -> Self {
        let q_x = (local_x.clamp(0.0, 64.0) / 64.0 * 63.0).round() as u32;
        let q_y = (local_y.clamp(0.0, 32.0) / 32.0 * 31.0).round() as u32;
        let q_z = (local_z.clamp(0.0, 64.0) / 64.0 * 63.0).round() as u32;
        Self {
            x: q_x.min(63) as u8,
            y: q_y.min(31) as u8,
            z: q_z.min(63) as u8,
        }
    }

    /// Constructs a horizon coordinate from fixed-point coordinates.
    pub fn quantize(local_x: Fixed64, local_y: Fixed64, local_z: Fixed64) -> Self {
        Self::from_f64(local_x.to_f64(), local_y.to_f64(), local_z.to_f64())
    }

    /// Dequantizes to continuous f64 coordinates (x, y, z).
    pub fn to_f64(self) -> (f64, f64, f64) {
        (
            (self.x as f64) * Self::HORIZONTAL_RESOLUTION_METERS,
            (self.y as f64) * Self::VERTICAL_RESOLUTION_METERS,
            (self.z as f64) * Self::HORIZONTAL_RESOLUTION_METERS,
        )
    }

    /// Dequantizes to fixed-point Vec3Fix.
    pub fn dequantize(self) -> Vec3Fix {
        let (x, y, z) = self.to_f64();
        Vec3Fix::from_f64(x, y, z)
    }

    /// Packs horizon coordinate, 4-bit heading, and 3-bit flags into an exact 3-byte wire buffer.
    pub fn pack(self, heading: QuantizedYaw4Bit, flags: u8) -> [u8; 3] {
        let x_val = self.x & 0x3F;
        let z_val = self.z & 0x3F;
        let y_val = self.y & 0x1F;
        let heading_val = heading.0 & 0x0F;
        let flags_val = flags & 0x07;

        [
            x_val | ((z_val & 0x03) << 6),
            ((z_val >> 2) & 0x0F) | ((y_val & 0x0F) << 4),
            ((y_val >> 4) & 0x01) | (heading_val << 1) | (flags_val << 5),
        ]
    }

    /// Unpacks horizon coordinate, 4-bit heading, and 3-bit flags from a 3-byte buffer.
    pub fn unpack(bytes: [u8; 3]) -> (Self, QuantizedYaw4Bit, u8) {
        let x = bytes[0] & 0x3F;
        let z = ((bytes[0] >> 6) & 0x03) | ((bytes[1] & 0x0F) << 2);
        let y = ((bytes[1] >> 4) & 0x0F) | ((bytes[2] & 0x01) << 4);
        let heading = (bytes[2] >> 1) & 0x0F;
        let flags = (bytes[2] >> 5) & 0x07;

        (Self { x, y, z }, QuantizedYaw4Bit(heading), flags)
    }
}
