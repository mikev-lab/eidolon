//! Hierarchical continental and planetary spatial addressing.
//!
//! Eliminates floating-point drift across massive continuous worlds (100 to 2,500+ km² and beyond)
//! by combining discrete 32-bit sector indices with 32.32 fixed-point local offsets (`Vec3Fix`).
//! World dimensions can exceed 500,000,000 kilometers with zero precision loss.

use crate::fixed::{Fixed64, Vec3Fix};

/// Edge length of a continental spatial sector in meters (256.0m x 256.0m).
pub const SECTOR_EDGE_METERS: f64 = 256.0;

/// Fixed-point representation of the sector edge length.
pub const SECTOR_EDGE_FIXED: Fixed64 = Fixed64::from_i32(256);

/// Hierarchical global coordinate representation for continental and planetary scale.
///
/// Combines a 2D discrete sector coordinate `(sector_x, sector_z)` with a local
/// continuous 32.32 fixed-point position `offset` within that sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GlobalCoord {
    /// Discrete horizontal sector index along the X axis.
    pub sector_x: i32,
    /// Discrete horizontal sector index along the Z axis.
    pub sector_z: i32,
    /// Local continuous position within the sector.
    /// `offset.x` is normalized to `[0, 256.0)`, `offset.z` is normalized to `[0, 256.0)`,
    /// and `offset.y` represents vertical elevation in meters.
    pub offset: Vec3Fix,
}

impl GlobalCoord {
    /// Creates a new `GlobalCoord`, automatically normalizing offsets into proper sectors.
    pub fn new(sector_x: i32, sector_z: i32, offset: Vec3Fix) -> Self {
        let mut coord = Self {
            sector_x,
            sector_z,
            offset,
        };
        coord.normalize();
        coord
    }

    /// Creates a `GlobalCoord` from continuous continuous coordinates in meters.
    pub fn from_continuous(x: Fixed64, y: Fixed64, z: Fixed64) -> Self {
        let mut coord = Self {
            sector_x: 0,
            sector_z: 0,
            offset: Vec3Fix { x, y, z },
        };
        coord.normalize();
        coord
    }

    /// Creates a `GlobalCoord` from discrete sector indices and local floating-point meters.
    pub fn from_sector_and_local(
        sector_x: i32,
        sector_z: i32,
        local_x_m: f64,
        local_y_m: f64,
        local_z_m: f64,
    ) -> Self {
        Self::new(
            sector_x,
            sector_z,
            Vec3Fix::from_f64(local_x_m, local_y_m, local_z_m),
        )
    }

    /// Normalizes the local offset so that `0 <= offset.x < 256.0` and `0 <= offset.z < 256.0`.
    pub fn normalize(&mut self) {
        // Bit-exact branchless Euclidean sector normalization across arbitrary positive and negative ranges.
        // In 32.32 fixed-point representation, each 256m sector spans 2^8 * 2^32 = 2^40 raw units.
        let shift_x = (self.offset.x.raw() >> 40) as i32;
        if shift_x != 0 {
            self.sector_x = self.sector_x.wrapping_add(shift_x);
            self.offset.x -= Fixed64::from_raw((shift_x as i64) << 40);
        }

        let shift_z = (self.offset.z.raw() >> 40) as i32;
        if shift_z != 0 {
            self.sector_z = self.sector_z.wrapping_add(shift_z);
            self.offset.z -= Fixed64::from_raw((shift_z as i64) << 40);
        }
    }

    /// Computes the relative vector displacement `(target - self)` in meters.
    ///
    /// Accurate for observable entity ranges (up to several thousand meters).
    pub fn displacement_to(&self, target: &Self) -> Vec3Fix {
        let dx_sectors = (target.sector_x as i64) - (self.sector_x as i64);
        let dz_sectors = (target.sector_z as i64) - (self.sector_z as i64);

        // Saturate sector displacement to prevent i32 overflow across planetary extremes
        let dx_meters = dx_sectors
            .saturating_mul(256)
            .clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        let dz_meters = dz_sectors
            .saturating_mul(256)
            .clamp(i32::MIN as i64, i32::MAX as i64) as i32;

        let dx = Fixed64::from_i32(dx_meters) + (target.offset.x - self.offset.x);
        let dy = target.offset.y - self.offset.y;
        let dz = Fixed64::from_i32(dz_meters) + (target.offset.z - self.offset.z);

        Vec3Fix {
            x: dx,
            y: dy,
            z: dz,
        }
    }

    /// Computes the exact squared horizontal distance between two global coordinates.
    pub fn horizontal_distance_squared(&self, other: &Self) -> Fixed64 {
        let disp = self.displacement_to(other);
        (disp.x * disp.x) + (disp.z * disp.z)
    }

    /// Computes the exact squared 3D Euclidean distance between two global coordinates.
    pub fn distance_squared(&self, other: &Self) -> Fixed64 {
        let disp = self.displacement_to(other);
        (disp.x * disp.x) + (disp.y * disp.y) + (disp.z * disp.z)
    }

    /// Translates this coordinate by a relative continuous displacement vector.
    pub fn translate(&self, delta: Vec3Fix) -> Self {
        Self::new(self.sector_x, self.sector_z, self.offset + delta)
    }

    /// Converts this global coordinate to a continuous 3D fixed-point coordinate.
    ///
    /// Accurate for continuous positions within ±2,147,483 kilometers of the world origin.
    #[inline]
    pub fn to_continuous(&self) -> Vec3Fix {
        let sec_x = (self.sector_x as i64) * 256;
        let sec_z = (self.sector_z as i64) * 256;
        Vec3Fix {
            x: Fixed64::from_raw(sec_x << Fixed64::FRACTIONAL_BITS) + self.offset.x,
            y: self.offset.y,
            z: Fixed64::from_raw(sec_z << Fixed64::FRACTIONAL_BITS) + self.offset.z,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_coord_normalization_positive_and_negative() {
        // Position within sector (no sector shift)
        let c1 = GlobalCoord::new(10, 20, Vec3Fix::from_f64(100.0, 5.0, 200.0));
        assert_eq!(c1.sector_x, 10);
        assert_eq!(c1.sector_z, 20);
        assert_eq!(c1.offset.x, Fixed64::from_f64(100.0));

        // Shift positive X by 300m (> 256m)
        let c2 = GlobalCoord::new(10, 20, Vec3Fix::from_f64(300.0, 5.0, 10.0));
        assert_eq!(c2.sector_x, 11);
        assert_eq!(c2.offset.x, Fixed64::from_f64(44.0));

        // Shift negative X by -10m (< 0m)
        let c3 = GlobalCoord::new(10, 20, Vec3Fix::from_f64(-10.0, 5.0, 10.0));
        assert_eq!(c3.sector_x, 9);
        assert_eq!(c3.offset.x, Fixed64::from_f64(246.0));

        // Shift large negative Z by -600m
        let c4 = GlobalCoord::new(10, 20, Vec3Fix::from_f64(10.0, 5.0, -600.0));
        // -600m = -2 * 256 (-512) - 88m => shift = -3 sectors, offset = 256 - 88 = 168m
        assert_eq!(c4.sector_z, 17);
        assert_eq!(c4.offset.z, Fixed64::from_f64(168.0));
    }

    #[test]
    fn test_global_coord_displacement_across_sectors() {
        let p1 = GlobalCoord::new(0, 0, Vec3Fix::from_f64(50.0, 0.0, 50.0));
        let p2 = GlobalCoord::new(2, 3, Vec3Fix::from_f64(100.0, 0.0, 150.0));

        // p2 is 2 sectors East (512m) + (100 - 50) = 562m along X
        // p2 is 3 sectors South (768m) + (150 - 50) = 868m along Z
        let disp = p1.displacement_to(&p2);
        assert_eq!(disp.x, Fixed64::from_f64(562.0));
        assert_eq!(disp.z, Fixed64::from_f64(868.0));

        // Inverse displacement is exactly negated
        let inv = p2.displacement_to(&p1);
        assert_eq!(inv.x, Fixed64::from_f64(-562.0));
        assert_eq!(inv.z, Fixed64::from_f64(-868.0));
    }

    #[test]
    fn test_global_coord_planetary_distance_10_000_km() {
        // Sector index for 10,000 km: 10,000,000m / 256m = 39,062.5 sectors
        let p_origin = GlobalCoord::new(0, 0, Vec3Fix::ZERO);
        let p_distant = GlobalCoord::new(39_062, 0, Vec3Fix::from_f64(128.0, 0.0, 0.0));

        // 39,062 * 256 + 128 = 9,999,872 + 128 = 10,000,000 meters exactly
        let disp = p_origin.displacement_to(&p_distant);
        assert_eq!(disp.x.to_i32(), 10_000_000);
        assert_eq!(disp.z.to_i32(), 0);
    }

    #[test]
    fn test_global_coord_normalization_extreme_bounds() {
        // Test Fixed64::MIN without negation overflow panic on i32::MIN
        let coord_min = GlobalCoord::from_continuous(Fixed64::MIN, Fixed64::ZERO, Fixed64::MIN);
        assert!(coord_min.offset.x >= Fixed64::ZERO);
        assert!(coord_min.offset.x < Fixed64::from_i32(256));
        assert!(coord_min.offset.z >= Fixed64::ZERO);
        assert!(coord_min.offset.z < Fixed64::from_i32(256));

        // Test Fixed64::MAX
        let coord_max = GlobalCoord::from_continuous(Fixed64::MAX, Fixed64::ZERO, Fixed64::MAX);
        assert!(coord_max.offset.x >= Fixed64::ZERO);
        assert!(coord_max.offset.x < Fixed64::from_i32(256));
        assert!(coord_max.offset.z >= Fixed64::ZERO);
        assert!(coord_max.offset.z < Fixed64::from_i32(256));

        // Test multi-million sector displacement saturation without i32 overflow
        let c1 = GlobalCoord::new(0, 0, Vec3Fix::ZERO);
        let c2 = GlobalCoord::new(10_000_000, 0, Vec3Fix::ZERO);
        let disp = c1.displacement_to(&c2);
        assert!(disp.x > Fixed64::ZERO);
    }
}
