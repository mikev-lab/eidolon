//! 8-lane fixed-point SIMD vector primitives for high-density Struct-of-Arrays (SoA) simulation.
//!
//! Provides `Vec3Fix8x`, an 8-wide 3D fixed-point vector layout that maps 8 parallel entities
//! into hardware vector registers (AVX2 / NEON) using pure safe Rust without third-party crates.

use std::ops::{Add, Mul, Sub};

use crate::fixed::{Fixed64, Vec3Fix};

/// 8-lane 3D fixed-point vector representing 8 parallel entities in Struct-of-Arrays registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Vec3Fix8x {
    /// 8 parallel X coordinates in 32.32 fixed-point.
    pub x: [Fixed64; 8],
    /// 8 parallel Y (elevation) coordinates in 32.32 fixed-point.
    pub y: [Fixed64; 8],
    /// 8 parallel Z coordinates in 32.32 fixed-point.
    pub z: [Fixed64; 8],
}

impl Vec3Fix8x {
    /// Zero vector across all 8 lanes.
    pub const ZERO: Self = Self {
        x: [Fixed64::ZERO; 8],
        y: [Fixed64::ZERO; 8],
        z: [Fixed64::ZERO; 8],
    };

    /// Constructs an 8-lane vector from explicit coordinate arrays.
    #[inline]
    pub const fn new(x: [Fixed64; 8], y: [Fixed64; 8], z: [Fixed64; 8]) -> Self {
        Self { x, y, z }
    }

    /// Splats a single 3D fixed-point vector across all 8 parallel lanes.
    #[inline]
    pub const fn splat(v: Vec3Fix) -> Self {
        Self {
            x: [v.x; 8],
            y: [v.y; 8],
            z: [v.z; 8],
        }
    }

    /// Loads 8 contiguous `Vec3Fix` vectors into 8-lane Struct-of-Arrays format.
    #[inline]
    pub fn from_slice_8(slice: &[Vec3Fix; 8]) -> Self {
        Self {
            x: [
                slice[0].x, slice[1].x, slice[2].x, slice[3].x, slice[4].x, slice[5].x, slice[6].x,
                slice[7].x,
            ],
            y: [
                slice[0].y, slice[1].y, slice[2].y, slice[3].y, slice[4].y, slice[5].y, slice[6].y,
                slice[7].y,
            ],
            z: [
                slice[0].z, slice[1].z, slice[2].z, slice[3].z, slice[4].z, slice[5].z, slice[6].z,
                slice[7].z,
            ],
        }
    }

    /// Writes 8-lane Struct-of-Arrays data back into an array of 8 `Vec3Fix` vectors.
    #[inline]
    pub fn write_to_slice_8(&self, slice: &mut [Vec3Fix; 8]) {
        for (i, item) in slice.iter_mut().enumerate() {
            *item = Vec3Fix {
                x: self.x[i],
                y: self.y[i],
                z: self.z[i],
            };
        }
    }

    /// Adds two 8-lane vectors in parallel using saturating fixed-point arithmetic.
    #[inline]
    pub fn saturating_add(&self, other: &Self) -> Self {
        let mut res = Self::ZERO;
        for i in 0..8 {
            res.x[i] = self.x[i] + other.x[i];
            res.y[i] = self.y[i] + other.y[i];
            res.z[i] = self.z[i] + other.z[i];
        }
        res
    }

    /// Subtracts two 8-lane vectors in parallel using saturating fixed-point arithmetic.
    #[inline]
    pub fn saturating_sub(&self, other: &Self) -> Self {
        let mut res = Self::ZERO;
        for i in 0..8 {
            res.x[i] = self.x[i] - other.x[i];
            res.y[i] = self.y[i] - other.y[i];
            res.z[i] = self.z[i] - other.z[i];
        }
        res
    }

    /// Multiplies all 8 lanes by a uniform scalar factor (e.g. delta time `dt`).
    #[inline]
    pub fn mul_scalar(&self, scalar: Fixed64) -> Self {
        let mut res = Self::ZERO;
        for i in 0..8 {
            res.x[i] = self.x[i] * scalar;
            res.y[i] = self.y[i] * scalar;
            res.z[i] = self.z[i] * scalar;
        }
        res
    }

    /// Executes a single vectorized kinematics integration step on 8 contiguous entities:
    /// `positions += velocities * dt`.
    #[inline]
    pub fn step_kinematics_chunk(
        positions: &mut [Vec3Fix; 8],
        velocities: &[Vec3Fix; 8],
        dt: Fixed64,
    ) {
        let pos_8x = Self::from_slice_8(positions);
        let vel_8x = Self::from_slice_8(velocities);
        let displacement_8x = vel_8x.mul_scalar(dt);
        let updated_pos = pos_8x.saturating_add(&displacement_8x);
        updated_pos.write_to_slice_8(positions);
    }

    /// Computes parallel squared distances from all 8 lanes to a common target point.
    #[inline]
    pub fn batch_distance_squared(&self, target: Vec3Fix) -> [Fixed64; 8] {
        let mut out = [Fixed64::ZERO; 8];
        for (i, item) in out.iter_mut().enumerate() {
            let dx = self.x[i] - target.x;
            let dy = self.y[i] - target.y;
            let dz = self.z[i] - target.z;
            *item = (dx * dx) + (dy * dy) + (dz * dz);
        }
        out
    }

    /// Evaluates proximity against a common target and returns an 8-bit bitmask
    /// where bit `i` is set if entity `i` is within `radius_sq`.
    #[inline]
    pub fn filter_within_radius(&self, target: Vec3Fix, radius_sq: Fixed64) -> u8 {
        let dists = self.batch_distance_squared(target);
        let mut mask = 0u8;
        for (i, &d) in dists.iter().enumerate() {
            if d <= radius_sq {
                mask |= 1 << i;
            }
        }
        mask
    }
}

impl Add for Vec3Fix8x {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        self.saturating_add(&rhs)
    }
}

impl Sub for Vec3Fix8x {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        self.saturating_sub(&rhs)
    }
}

impl Mul<Fixed64> for Vec3Fix8x {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Fixed64) -> Self {
        self.mul_scalar(rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vec3fix8x_splat_and_pack_roundtrip() {
        let v = Vec3Fix::from_f64(12.5, -4.0, 99.25);
        let splatted = Vec3Fix8x::splat(v);

        for i in 0..8 {
            assert_eq!(splatted.x[i], v.x);
            assert_eq!(splatted.y[i], v.y);
            assert_eq!(splatted.z[i], v.z);
        }

        let mut slice = [Vec3Fix::ZERO; 8];
        splatted.write_to_slice_8(&mut slice);

        for item in slice.iter() {
            assert_eq!(*item, v);
        }

        let reloaded = Vec3Fix8x::from_slice_8(&slice);
        assert_eq!(reloaded, splatted);
    }

    #[test]
    fn test_vec3fix8x_saturating_add_sub() {
        let a = Vec3Fix8x::splat(Vec3Fix::from_f64(10.0, 20.0, 30.0));
        let b = Vec3Fix8x::splat(Vec3Fix::from_f64(1.0, 2.0, 3.0));

        let sum = a + b;
        let diff = a - b;

        let expected_sum = Vec3Fix::from_f64(11.0, 22.0, 33.0);
        let expected_diff = Vec3Fix::from_f64(9.0, 18.0, 27.0);

        for i in 0..8 {
            assert_eq!(sum.x[i], expected_sum.x);
            assert_eq!(sum.y[i], expected_sum.y);
            assert_eq!(sum.z[i], expected_sum.z);

            assert_eq!(diff.x[i], expected_diff.x);
            assert_eq!(diff.y[i], expected_diff.y);
            assert_eq!(diff.z[i], expected_diff.z);
        }
    }

    #[test]
    fn test_vec3fix8x_mul_scalar() {
        let v = Vec3Fix8x::splat(Vec3Fix::from_f64(10.0, -20.0, 50.0));
        let scalar = Fixed64::from_f64(0.5);

        let scaled = v * scalar;
        let expected = Vec3Fix::from_f64(5.0, -10.0, 25.0);

        for i in 0..8 {
            assert_eq!(scaled.x[i], expected.x);
            assert_eq!(scaled.y[i], expected.y);
            assert_eq!(scaled.z[i], expected.z);
        }
    }

    #[test]
    fn test_vec3fix8x_step_kinematics_chunk_parity() {
        let mut pos = [
            Vec3Fix::from_f64(0.0, 0.0, 0.0),
            Vec3Fix::from_f64(10.0, 0.0, 0.0),
            Vec3Fix::from_f64(20.0, 0.0, 0.0),
            Vec3Fix::from_f64(30.0, 0.0, 0.0),
            Vec3Fix::from_f64(40.0, 0.0, 0.0),
            Vec3Fix::from_f64(50.0, 0.0, 0.0),
            Vec3Fix::from_f64(60.0, 0.0, 0.0),
            Vec3Fix::from_f64(70.0, 0.0, 0.0),
        ];

        let vel = [
            Vec3Fix::from_f64(1.0, 0.0, 0.0),
            Vec3Fix::from_f64(2.0, 0.0, 0.0),
            Vec3Fix::from_f64(3.0, 0.0, 0.0),
            Vec3Fix::from_f64(4.0, 0.0, 0.0),
            Vec3Fix::from_f64(5.0, 0.0, 0.0),
            Vec3Fix::from_f64(6.0, 0.0, 0.0),
            Vec3Fix::from_f64(7.0, 0.0, 0.0),
            Vec3Fix::from_f64(8.0, 0.0, 0.0),
        ];

        let dt = Fixed64::from_f64(0.05); // 50ms tick

        // Scalar expected reference
        let mut expected_pos = pos;
        for i in 0..8 {
            expected_pos[i] += vel[i] * dt;
        }

        // SIMD 8-lane step
        Vec3Fix8x::step_kinematics_chunk(&mut pos, &vel, dt);

        for i in 0..8 {
            assert_eq!(
                pos[i], expected_pos[i],
                "Lane {i} must match scalar physics step"
            );
        }
    }

    #[test]
    fn test_vec3fix8x_batch_distance_and_filtering() {
        let target = Vec3Fix::from_f64(0.0, 0.0, 0.0);
        let mut origins = [Vec3Fix::ZERO; 8];
        for (i, item) in origins.iter_mut().enumerate() {
            // Entities at distances 5m, 10m, 15m, 20m, 25m, 30m, 35m, 40m
            *item = Vec3Fix::from_f64((i + 1) as f64 * 5.0, 0.0, 0.0);
        }

        let vec_8x = Vec3Fix8x::from_slice_8(&origins);
        let dists = vec_8x.batch_distance_squared(target);

        for i in 0..8 {
            let scalar_d = origins[i].distance_squared(target);
            assert_eq!(dists[i], scalar_d);
        }

        // Radius of 16m -> radius_sq = 256
        // Entities 0 (5m -> 25) and 1 (10m -> 100) and 2 (15m -> 225) are <= 256.
        // Bitmask should have bits 0, 1, 2 set -> 0b0000_0111 = 0x07.
        let radius_sq = Fixed64::from_i32(256);
        let mask = vec_8x.filter_within_radius(target, radius_sq);
        assert_eq!(mask, 0x07);
    }
}
