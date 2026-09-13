//! Deterministic 32.32 fixed-point integer mathematics and vector primitives.
//!
//! Designed for cross-platform bit-exact simulation with zero floating-point non-determinism.

use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

/// 32.32 signed fixed-point integer.
///
/// Allocates 32 bits for the integer component (approx. +/- 2.14 billion)
/// and 32 bits for fractional precision (approx. 0.233 nanometer resolution).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
#[repr(transparent)]
pub struct Fixed64(pub i64);

impl Fixed64 {
    /// Number of fractional bits in the 32.32 representation.
    pub const FRACTIONAL_BITS: u32 = 32;

    /// Scaling factor: 2^32.
    pub const SCALE: i64 = 1i64 << Self::FRACTIONAL_BITS;

    /// Constant representing 0.
    pub const ZERO: Self = Self(0);

    /// Constant representing 1.0.
    pub const ONE: Self = Self(Self::SCALE);

    /// Constant representing 0.5.
    pub const HALF: Self = Self(Self::SCALE >> 1);

    /// Constant representing the smallest representable positive delta (1 / 2^32).
    pub const EPSILON: Self = Self(1);

    /// Minimum representable value.
    pub const MIN: Self = Self(i64::MIN);

    /// Maximum representable value.
    pub const MAX: Self = Self(i64::MAX);

    /// Constructs a fixed-point value from raw underlying integer.
    #[inline]
    pub const fn from_raw(raw: i64) -> Self {
        Self(raw)
    }

    /// Returns the raw 64-bit integer.
    #[inline]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Constructs a fixed-point value from an integer.
    #[inline]
    pub const fn from_i32(val: i32) -> Self {
        Self((val as i64) << Self::FRACTIONAL_BITS)
    }

    /// Truncates the fixed-point number to an integer.
    #[inline]
    pub const fn to_i32(self) -> i32 {
        (self.0 >> Self::FRACTIONAL_BITS) as i32
    }

    /// Returns the largest integer less than or equal to this Fixed64.
    #[inline]
    pub const fn floor(self) -> Self {
        Self((self.0 >> Self::FRACTIONAL_BITS) << Self::FRACTIONAL_BITS)
    }

    /// Returns the smallest integer greater than or equal to this Fixed64.
    #[inline]
    pub const fn ceil(self) -> Self {
        let mask = (1i64 << Self::FRACTIONAL_BITS) - 1;
        if (self.0 & mask) == 0 {
            self
        } else {
            Self(((self.0 >> Self::FRACTIONAL_BITS) + 1) << Self::FRACTIONAL_BITS)
        }
    }

    /// Converts an f64 to Fixed64, clamping to representable bounds.
    #[inline]
    pub fn from_f64(val: f64) -> Self {
        let scaled = val * (Self::SCALE as f64);
        if scaled >= (i64::MAX as f64) {
            Self::MAX
        } else if scaled <= (i64::MIN as f64) {
            Self::MIN
        } else {
            Self(scaled as i64)
        }
    }

    /// Converts this Fixed64 to an f64.
    #[inline]
    pub fn to_f64(self) -> f64 {
        (self.0 as f64) / (Self::SCALE as f64)
    }

    /// Converts an f32 to Fixed64.
    #[inline]
    pub fn from_f32(val: f32) -> Self {
        Self::from_f64(val as f64)
    }

    /// Converts this Fixed64 to an f32.
    #[inline]
    pub fn to_f32(self) -> f32 {
        self.to_f64() as f32
    }

    /// Saturating addition.
    #[inline]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    /// Saturating subtraction.
    #[inline]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }

    /// Saturating multiplication using 128-bit intermediate integer to avoid overflow.
    #[inline]
    pub const fn saturating_mul(self, rhs: Self) -> Self {
        let wide = (self.0 as i128) * (rhs.0 as i128);
        let res = wide >> Self::FRACTIONAL_BITS;
        if res > (i64::MAX as i128) {
            Self::MAX
        } else if res < (i64::MIN as i128) {
            Self::MIN
        } else {
            Self(res as i64)
        }
    }

    /// Saturating division using 128-bit intermediate integer to avoid overflow.
    #[inline]
    pub const fn saturating_div(self, rhs: Self) -> Self {
        if rhs.0 == 0 {
            if self.0 >= 0 {
                return Self::MAX;
            } else {
                return Self::MIN;
            }
        }
        let wide = (self.0 as i128) << Self::FRACTIONAL_BITS;
        let res = wide / (rhs.0 as i128);
        if res > (i64::MAX as i128) {
            Self::MAX
        } else if res < (i64::MIN as i128) {
            Self::MIN
        } else {
            Self(res as i64)
        }
    }

    /// Absolute value with saturation at i64::MIN.
    #[inline]
    pub const fn abs(self) -> Self {
        Self(self.0.saturating_abs())
    }

    /// Clamps value between min and max bounds branchlessly using hardware conditional move.
    #[inline]
    pub fn clamp(self, min: Self, max: Self) -> Self {
        Self(self.0.clamp(min.0, max.0))
    }

    /// Returns the minimum of self and other.
    #[inline]
    pub const fn min(self, other: Self) -> Self {
        if self.0 < other.0 {
            self
        } else {
            other
        }
    }

    /// Returns the maximum of self and other.
    #[inline]
    pub const fn max(self, other: Self) -> Self {
        if self.0 > other.0 {
            self
        } else {
            other
        }
    }

    /// Deterministic integer square root for 32.32 fixed point.
    ///
    /// Evaluates sqrt(x * 2^-32) = isqrt(x * 2^32) * 2^-32.
    /// Returns ZERO for non-positive inputs.
    pub fn sqrt(self) -> Self {
        if self.0 <= 0 {
            return Self::ZERO;
        }
        let val = (self.0 as u128) << Self::FRACTIONAL_BITS;
        // Integer square root via bit-by-bit binary search
        let mut root: u128 = 0;
        let mut bit: u128 = 1u128 << 126;

        while bit > val {
            bit >>= 2;
        }

        let mut remainder = val;
        while bit != 0 {
            if remainder >= root + bit {
                remainder -= root + bit;
                root = (root >> 1) + bit;
            } else {
                root >>= 1;
            }
            bit >>= 2;
        }

        if root > (i64::MAX as u128) {
            Self::MAX
        } else {
            Self(root as i64)
        }
    }
}

impl Add for Fixed64 {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        self.saturating_add(rhs)
    }
}

impl AddAssign for Fixed64 {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        *self = self.saturating_add(rhs);
    }
}

impl Sub for Fixed64 {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        self.saturating_sub(rhs)
    }
}

impl SubAssign for Fixed64 {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        *self = self.saturating_sub(rhs);
    }
}

impl Mul for Fixed64 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        self.saturating_mul(rhs)
    }
}

impl MulAssign for Fixed64 {
    #[inline]
    fn mul_assign(&mut self, rhs: Self) {
        *self = self.saturating_mul(rhs);
    }
}

impl Div for Fixed64 {
    type Output = Self;
    #[inline]
    fn div(self, rhs: Self) -> Self {
        self.saturating_div(rhs)
    }
}

impl DivAssign for Fixed64 {
    #[inline]
    fn div_assign(&mut self, rhs: Self) {
        *self = self.saturating_div(rhs);
    }
}

impl Neg for Fixed64 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self(self.0.saturating_neg())
    }
}

/// Fixed-point 3D vector.
///
/// Designed for deterministic spatial operations without floating-point drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Vec3Fix {
    /// Horizontal X coordinate.
    pub x: Fixed64,
    /// Vertical elevation Y coordinate.
    pub y: Fixed64,
    /// Horizontal Z coordinate.
    pub z: Fixed64,
}

impl Vec3Fix {
    /// Zero vector (0, 0, 0).
    pub const ZERO: Self = Self {
        x: Fixed64::ZERO,
        y: Fixed64::ZERO,
        z: Fixed64::ZERO,
    };

    /// Unit up vector (0, 1, 0).
    pub const UP: Self = Self {
        x: Fixed64::ZERO,
        y: Fixed64::ONE,
        z: Fixed64::ZERO,
    };

    /// Unit forward vector (0, 0, 1).
    pub const FORWARD: Self = Self {
        x: Fixed64::ZERO,
        y: Fixed64::ZERO,
        z: Fixed64::ONE,
    };

    /// Unit right vector (1, 0, 0).
    pub const RIGHT: Self = Self {
        x: Fixed64::ONE,
        y: Fixed64::ZERO,
        z: Fixed64::ZERO,
    };

    /// Constructs a vector from fixed-point coordinates.
    #[inline]
    pub const fn new(x: Fixed64, y: Fixed64, z: Fixed64) -> Self {
        Self { x, y, z }
    }

    /// Constructs a vector from discrete integer coordinates.
    #[inline]
    pub const fn from_i32(x: i32, y: i32, z: i32) -> Self {
        Self {
            x: Fixed64::from_i32(x),
            y: Fixed64::from_i32(y),
            z: Fixed64::from_i32(z),
        }
    }

    /// Constructs a vector from f64 coordinates.
    #[inline]
    pub fn from_f64(x: f64, y: f64, z: f64) -> Self {
        Self {
            x: Fixed64::from_f64(x),
            y: Fixed64::from_f64(y),
            z: Fixed64::from_f64(z),
        }
    }

    /// Converts the vector to f64 tuple (x, y, z).
    #[inline]
    pub fn to_f64(self) -> (f64, f64, f64) {
        (self.x.to_f64(), self.y.to_f64(), self.z.to_f64())
    }

    /// Scales vector by a scalar Fixed64.
    #[inline]
    pub fn scale(self, factor: Fixed64) -> Self {
        Self {
            x: self.x * factor,
            y: self.y * factor,
            z: self.z * factor,
        }
    }

    /// Computes the dot product between two vectors.
    #[inline]
    pub fn dot(self, rhs: Self) -> Fixed64 {
        (self.x * rhs.x) + (self.y * rhs.y) + (self.z * rhs.z)
    }

    /// Computes the cross product between two vectors.
    #[inline]
    pub fn cross(self, rhs: Self) -> Self {
        Self {
            x: (self.y * rhs.z) - (self.z * rhs.y),
            y: (self.z * rhs.x) - (self.x * rhs.z),
            z: (self.x * rhs.y) - (self.y * rhs.x),
        }
    }

    /// Returns the squared Euclidean magnitude.
    ///
    /// Essential for performance: allows distance comparisons without costly square roots.
    #[inline]
    pub fn magnitude_squared(self) -> Fixed64 {
        self.dot(self)
    }

    /// Returns the exact Euclidean magnitude.
    #[inline]
    pub fn magnitude(self) -> Fixed64 {
        self.magnitude_squared().sqrt()
    }

    /// Returns the squared distance between two points.
    #[inline]
    pub fn distance_squared(self, other: Self) -> Fixed64 {
        (self - other).magnitude_squared()
    }

    /// Computes the squared distances from 4 origin points to a common target point in parallel.
    ///
    /// Structured as an unrolled 4-wide batch allowing LLVM to auto-vectorize
    /// arithmetic across SIMD vector registers (NEON / AVX2).
    #[inline]
    pub fn batch_distance_squared_4x(origins: [Self; 4], target: Self) -> [Fixed64; 4] {
        let dx0 = origins[0].x - target.x;
        let dy0 = origins[0].y - target.y;
        let dz0 = origins[0].z - target.z;

        let dx1 = origins[1].x - target.x;
        let dy1 = origins[1].y - target.y;
        let dz1 = origins[1].z - target.z;

        let dx2 = origins[2].x - target.x;
        let dy2 = origins[2].y - target.y;
        let dz2 = origins[2].z - target.z;

        let dx3 = origins[3].x - target.x;
        let dy3 = origins[3].y - target.y;
        let dz3 = origins[3].z - target.z;

        [
            (dx0 * dx0) + (dy0 * dy0) + (dz0 * dz0),
            (dx1 * dx1) + (dy1 * dy1) + (dz1 * dz1),
            (dx2 * dx2) + (dy2 * dy2) + (dz2 * dz2),
            (dx3 * dx3) + (dy3 * dy3) + (dz3 * dz3),
        ]
    }

    /// Returns the Euclidean distance between two points.
    #[inline]
    pub fn distance(self, other: Self) -> Fixed64 {
        (self - other).magnitude()
    }

    /// Returns the Manhattan distance (L1 norm).
    #[inline]
    pub fn manhattan_distance(self, other: Self) -> Fixed64 {
        (self.x - other.x).abs() + (self.y - other.y).abs() + (self.z - other.z).abs()
    }

    /// Normalizes the vector to unit length, or returns ZERO if magnitude is negligible.
    pub fn normalize_or_zero(self) -> Self {
        let mag = self.magnitude();
        if mag <= Fixed64::EPSILON {
            Self::ZERO
        } else {
            Self {
                x: self.x / mag,
                y: self.y / mag,
                z: self.z / mag,
            }
        }
    }
}

impl Add for Vec3Fix {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl AddAssign for Vec3Fix {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
        self.z += rhs.z;
    }
}

impl Sub for Vec3Fix {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

impl SubAssign for Vec3Fix {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
        self.z -= rhs.z;
    }
}

impl Mul<Fixed64> for Vec3Fix {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Fixed64) -> Self {
        self.scale(rhs)
    }
}

impl Div<Fixed64> for Vec3Fix {
    type Output = Self;
    #[inline]
    fn div(self, rhs: Fixed64) -> Self {
        Self {
            x: self.x / rhs,
            y: self.y / rhs,
            z: self.z / rhs,
        }
    }
}

impl Neg for Vec3Fix {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }
}
