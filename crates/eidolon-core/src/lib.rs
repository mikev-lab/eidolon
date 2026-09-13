//! Core game mathematics, fixed-point vectors, coordinate quantization, and dead reckoning extrapolation algorithms.
//!
//! `eidolon-core` is a foundational, standalone crate designed with zero runtime dependencies.
//! It can be compiled for server runtimes or embedded into client game engines for deterministic prediction.

#![deny(unsafe_code)]
#![warn(missing_docs)]

/// Fixed-point numerical representations and deterministic vector primitives.
pub mod fixed {
    /// 32.32 fixed-point integer representation for deterministic cross-platform simulation.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
    pub struct Fixed64(pub i64);

    impl Fixed64 {
        /// Scaling factor: 2^32.
        pub const FRACTIONAL_BITS: u32 = 32;
        /// Zero constant.
        pub const ZERO: Self = Self(0);
        /// One constant.
        pub const ONE: Self = Self(1 << Self::FRACTIONAL_BITS);

        /// Creates a fixed-point value from an integer.
        #[inline]
        pub const fn from_i32(val: i32) -> Self {
            Self((val as i64) << Self::FRACTIONAL_BITS)
        }

        /// Returns the raw underlying 64-bit integer representation.
        #[inline]
        pub const fn raw(self) -> i64 {
            self.0
        }
    }

    /// Fixed-point 3D vector.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct Vec3Fix {
        /// X coordinate.
        pub x: Fixed64,
        /// Y coordinate (elevation).
        pub y: Fixed64,
        /// Z coordinate.
        pub z: Fixed64,
    }

    impl Vec3Fix {
        /// Zero vector constant.
        pub const ZERO: Self = Self {
            x: Fixed64::ZERO,
            y: Fixed64::ZERO,
            z: Fixed64::ZERO,
        };

        /// Creates a new vector with the given fixed-point coordinates.
        #[inline]
        pub const fn new(x: Fixed64, y: Fixed64, z: Fixed64) -> Self {
            Self { x, y, z }
        }
    }
}

/// Bounded integer coordinate and orientation quantization tables.
pub mod quant {
    /// Quantized 3D local cell-relative coordinates.
    ///
    /// Stores horizontal X and Z as 16-bit integers and elevation Y as a 12-bit integer,
    /// achieving sub-millimeter precision across a 64-meter cell.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct QuantizedCellCoord {
        /// Quantized X offset within the local cell (0..=65535).
        pub x: u16,
        /// Quantized vertical elevation within the vertical cell band (0..=4095).
        pub y: u16,
        /// Quantized Z offset within the local cell (0..=65535).
        pub z: u16,
    }

    /// Quantized discrete yaw heading (256 discrete angles covering 360 degrees).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct QuantizedYaw(pub u8);

    impl QuantizedYaw {
        /// Creates a quantized yaw from a single byte.
        #[inline]
        pub const fn from_byte(val: u8) -> Self {
            Self(val)
        }

        /// Returns the raw discrete angle byte.
        #[inline]
        pub const fn as_byte(self) -> u8 {
            self.0
        }
    }
}

/// Kinematics and intent-based dead reckoning state models.
pub mod kinematics {
    use crate::fixed::Vec3Fix;
    use crate::quant::QuantizedYaw;

    /// State descriptor representing entity movement intent.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct MovementIntent {
        /// Velocity vector expressed in fixed-point units per second.
        pub velocity: Vec3Fix,
        /// Current discrete facing direction.
        pub yaw: QuantizedYaw,
        /// Intent movement flags (e.g. running, jumping, crouched).
        pub flags: u8,
    }
}

#[cfg(test)]
mod tests {
    use super::fixed::{Fixed64, Vec3Fix};
    use super::quant::QuantizedYaw;

    #[test]
    fn test_fixed64_constants() {
        assert_eq!(Fixed64::ZERO.raw(), 0);
        assert_eq!(Fixed64::ONE.raw(), 1 << 32);
        assert_eq!(Fixed64::from_i32(5).raw(), 5 << 32);
    }

    #[test]
    fn test_vec3fix_zero() {
        let v = Vec3Fix::ZERO;
        assert_eq!(v.x, Fixed64::ZERO);
        assert_eq!(v.y, Fixed64::ZERO);
        assert_eq!(v.z, Fixed64::ZERO);
    }

    #[test]
    fn test_quantized_yaw_byte() {
        let yaw = QuantizedYaw::from_byte(128);
        assert_eq!(yaw.as_byte(), 128);
    }
}
