//! Multi-tier adaptive delta bitstream quantization for spatial transforms.
//!
//! Compresses high-frequency kinematic updates from 7 bytes down to 2 to 4 bytes
//! by transmitting signed relative offsets against client-acknowledged anchors.

use crate::quant::{QuantizedCellCoord, QuantizedYaw, FLAG_CELL_ANCHOR};

/// Step size for horizontal coordinates in Tier 1 (Small) delta (16 quantization units approx 15.6 mm).
pub const TIER1_HORIZONTAL_STEP: i32 = 16;

/// Step size for vertical coordinates in Tier 1 (Small) delta (2 vertical units approx 15.6 mm).
pub const TIER1_VERTICAL_STEP: i32 = 2;

/// Step size for yaw heading in Tier 1 (Small) delta (4 discrete steps approx 5.6 degrees).
pub const TIER1_YAW_STEP: i8 = 4;

/// Step size for horizontal coordinates in Tier 2 (Medium) delta (8 quantization units approx 7.8 mm).
pub const TIER2_HORIZONTAL_STEP: i32 = 8;

/// Step size for vertical coordinates in Tier 2 (Medium) delta (2 vertical units approx 15.6 mm).
pub const TIER2_VERTICAL_STEP: i32 = 2;

/// Identification tag for delta compression tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DeltaTier {
    /// Stationary entity: zero displacement payload (2-bit tag: 00).
    Stationary = 0b00,
    /// Small displacement: 16-bit payload (2-bit tag: 01 + 14 bits delta).
    Small = 0b01,
    /// Medium displacement: 32-bit payload (2-bit tag: 10 + 30 bits delta).
    Medium = 0b10,
    /// Full transform: absolute coordinate fallback (2-bit tag: 11 + 56 bits).
    Full = 0b11,
}

impl DeltaTier {
    /// Parses a 2-bit tag into a `DeltaTier`.
    #[inline]
    pub const fn from_tag(tag: u8) -> Self {
        match tag & 0x03 {
            0b00 => Self::Stationary,
            0b01 => Self::Small,
            0b10 => Self::Medium,
            _ => Self::Full,
        }
    }

    /// Returns the 2-bit tag representation.
    #[inline]
    pub const fn tag(self) -> u8 {
        self as u8
    }
}

/// Adaptive variable-bit delta transform update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaTransform {
    /// Entity is stationary within deadband limits.
    Stationary,
    /// Small movement (approx <0.12m displacement, <22 deg turn).
    /// Packed into exactly 16 bits (2 bytes) on the wire.
    Small {
        /// 4-bit signed horizontal X offset (-8..=7 steps of 16 units).
        dx: i8,
        /// 4-bit signed horizontal Z offset (-8..=7 steps of 16 units).
        dz: i8,
        /// 3-bit signed vertical Y offset (-4..=3 steps of 2 units).
        dy: i8,
        /// 3-bit signed yaw heading delta (-4..=3 steps of 4 discrete angles).
        dyaw: i8,
    },
    /// Medium movement (approx <1.0m displacement, full yaw update).
    /// Packed into exactly 32 bits (4 bytes) on the wire.
    Medium {
        /// 8-bit signed horizontal X offset (-128..=127 steps of 8 units).
        dx: i8,
        /// 8-bit signed horizontal Z offset (-128..=127 steps of 8 units).
        dz: i8,
        /// 6-bit signed vertical Y offset (-32..=31 steps of 2 units).
        dy: i8,
        /// Absolute 8-bit quantized yaw heading.
        yaw: QuantizedYaw,
    },
    /// Full 7-byte absolute transform fallback (cell crossing, teleport, large drift).
    Full {
        /// Absolute quantized cell coordinate.
        coord: QuantizedCellCoord,
        /// Absolute quantized yaw heading.
        yaw: QuantizedYaw,
        /// Transform flags (e.g. `FLAG_CELL_ANCHOR`).
        flags: u8,
    },
}

impl DeltaTransform {
    /// Returns the compression tier for this delta transform.
    #[inline]
    pub const fn tier(&self) -> DeltaTier {
        match self {
            Self::Stationary => DeltaTier::Stationary,
            Self::Small { .. } => DeltaTier::Small,
            Self::Medium { .. } => DeltaTier::Medium,
            Self::Full { .. } => DeltaTier::Full,
        }
    }

    /// Computes the optimal delta transform from a client baseline anchor to current state.
    pub fn compute(
        base_coord: QuantizedCellCoord,
        base_yaw: QuantizedYaw,
        target_coord: QuantizedCellCoord,
        target_yaw: QuantizedYaw,
        target_flags: u8,
    ) -> Self {
        // Any transform flags (like cell anchor crossings) require a Full update
        if target_flags & FLAG_CELL_ANCHOR != 0 {
            return Self::Full {
                coord: target_coord,
                yaw: target_yaw,
                flags: target_flags,
            };
        }

        let diff_x = target_coord.x as i32 - base_coord.x as i32;
        let diff_y = target_coord.y as i32 - base_coord.y as i32;
        let diff_z = target_coord.z as i32 - base_coord.z as i32;
        let diff_yaw = (target_yaw.as_byte() as i8).wrapping_sub(base_yaw.as_byte() as i8);

        // Check Stationary Tier (within deadband: <16 units horizontal, <2 units vertical, 0 yaw)
        if diff_x.abs() < TIER1_HORIZONTAL_STEP
            && diff_z.abs() < TIER1_HORIZONTAL_STEP
            && diff_y.abs() < TIER1_VERTICAL_STEP
            && diff_yaw == 0
        {
            return Self::Stationary;
        }

        // Check Small Tier (Tier 1)
        let s_dx = (diff_x + (TIER1_HORIZONTAL_STEP / 2) * diff_x.signum()) / TIER1_HORIZONTAL_STEP;
        let s_dz = (diff_z + (TIER1_HORIZONTAL_STEP / 2) * diff_z.signum()) / TIER1_HORIZONTAL_STEP;
        let s_dy = (diff_y + (TIER1_VERTICAL_STEP / 2) * diff_y.signum()) / TIER1_VERTICAL_STEP;
        let s_dyaw = (diff_yaw as i32 + (TIER1_YAW_STEP as i32 / 2) * (diff_yaw as i32).signum())
            / TIER1_YAW_STEP as i32;

        if (-8..=7).contains(&s_dx)
            && (-8..=7).contains(&s_dz)
            && (-4..=3).contains(&s_dy)
            && (-4..=3).contains(&s_dyaw)
        {
            return Self::Small {
                dx: s_dx as i8,
                dz: s_dz as i8,
                dy: s_dy as i8,
                dyaw: s_dyaw as i8,
            };
        }

        // Check Medium Tier (Tier 2)
        let m_dx = (diff_x + (TIER2_HORIZONTAL_STEP / 2) * diff_x.signum()) / TIER2_HORIZONTAL_STEP;
        let m_dz = (diff_z + (TIER2_HORIZONTAL_STEP / 2) * diff_z.signum()) / TIER2_HORIZONTAL_STEP;
        let m_dy = (diff_y + (TIER2_VERTICAL_STEP / 2) * diff_y.signum()) / TIER2_VERTICAL_STEP;

        if (-128..=127).contains(&m_dx)
            && (-128..=127).contains(&m_dz)
            && (-32..=31).contains(&m_dy)
        {
            return Self::Medium {
                dx: m_dx as i8,
                dz: m_dz as i8,
                dy: m_dy as i8,
                yaw: target_yaw,
            };
        }

        // Fallback to Full Transform (Tier 3)
        Self::Full {
            coord: target_coord,
            yaw: target_yaw,
            flags: target_flags,
        }
    }

    /// Reconstructs the updated position and heading by applying this delta to the baseline anchor.
    pub fn apply(
        &self,
        base_coord: QuantizedCellCoord,
        base_yaw: QuantizedYaw,
    ) -> (QuantizedCellCoord, QuantizedYaw, u8) {
        match *self {
            Self::Stationary => (base_coord, base_yaw, 0),
            Self::Small { dx, dz, dy, dyaw } => {
                let new_x = (base_coord.x as i32 + (dx as i32) * TIER1_HORIZONTAL_STEP)
                    .clamp(0, u16::MAX as i32) as u16;
                let new_z = (base_coord.z as i32 + (dz as i32) * TIER1_HORIZONTAL_STEP)
                    .clamp(0, u16::MAX as i32) as u16;
                let new_y =
                    (base_coord.y as i32 + (dy as i32) * TIER1_VERTICAL_STEP).clamp(0, 4095) as u16;
                let new_yaw_byte = base_yaw
                    .as_byte()
                    .wrapping_add((dyaw * TIER1_YAW_STEP) as u8);
                (
                    QuantizedCellCoord::new(new_x, new_y, new_z),
                    QuantizedYaw::from_byte(new_yaw_byte),
                    0,
                )
            }
            Self::Medium { dx, dz, dy, yaw } => {
                let new_x = (base_coord.x as i32 + (dx as i32) * TIER2_HORIZONTAL_STEP)
                    .clamp(0, u16::MAX as i32) as u16;
                let new_z = (base_coord.z as i32 + (dz as i32) * TIER2_HORIZONTAL_STEP)
                    .clamp(0, u16::MAX as i32) as u16;
                let new_y =
                    (base_coord.y as i32 + (dy as i32) * TIER2_VERTICAL_STEP).clamp(0, 4095) as u16;
                (QuantizedCellCoord::new(new_x, new_y, new_z), yaw, 0)
            }
            Self::Full { coord, yaw, flags } => (coord, yaw, flags),
        }
    }

    /// Packs this delta transform into an inline byte buffer.
    /// Returns the number of bytes written (1 to 8 bytes).
    pub fn pack_to_slice(&self, out: &mut [u8]) -> Option<usize> {
        match *self {
            Self::Stationary => {
                if out.is_empty() {
                    return None;
                }
                out[0] = DeltaTier::Stationary.tag();
                Some(1)
            }
            Self::Small { dx, dz, dy, dyaw } => {
                if out.len() < 2 {
                    return None;
                }
                // Tag: 2 bits [0..1] = 0b01
                // dx: 4 bits [2..5] (offset by +8 to unsigned 0..15)
                // dz: 4 bits [6..9] (offset by +8 to unsigned 0..15)
                // dy: 3 bits [10..12] (offset by +4 to unsigned 0..7)
                // dyaw: 3 bits [13..15] (offset by +4 to unsigned 0..7)
                let u_dx = (dx + 8) as u16 & 0x0F;
                let u_dz = (dz + 8) as u16 & 0x0F;
                let u_dy = (dy + 4) as u16 & 0x07;
                let u_dyaw = (dyaw + 4) as u16 & 0x07;

                let packed: u16 = (DeltaTier::Small.tag() as u16)
                    | (u_dx << 2)
                    | (u_dz << 6)
                    | (u_dy << 10)
                    | (u_dyaw << 13);

                out[0] = (packed & 0xFF) as u8;
                out[1] = ((packed >> 8) & 0xFF) as u8;
                Some(2)
            }
            Self::Medium { dx, dz, dy, yaw } => {
                if out.len() < 4 {
                    return None;
                }
                // Tag: 2 bits [0..1] = 0b10
                // dy: 6 bits [2..7] (offset by +32 to unsigned 0..63)
                // dx: 8 bits [8..15]
                // dz: 8 bits [16..23]
                // yaw: 8 bits [24..31]
                let u_dy = (dy + 32) as u8 & 0x3F;
                out[0] = DeltaTier::Medium.tag() | (u_dy << 2);
                out[1] = dx as u8;
                out[2] = dz as u8;
                out[3] = yaw.as_byte();
                Some(4)
            }
            Self::Full { coord, yaw, flags } => {
                if out.len() < 8 {
                    return None;
                }
                out[0] = DeltaTier::Full.tag() | ((flags & 0x0F) << 2);
                let full_7b = coord.pack_with_yaw_and_flags(yaw, flags);
                out[1..8].copy_from_slice(&full_7b);
                Some(8)
            }
        }
    }

    /// Unpacks a delta transform from a byte slice.
    pub fn unpack_from_slice(slice: &[u8]) -> Option<(Self, usize)> {
        if slice.is_empty() {
            return None;
        }

        let tag = DeltaTier::from_tag(slice[0]);
        match tag {
            DeltaTier::Stationary => Some((Self::Stationary, 1)),
            DeltaTier::Small => {
                if slice.len() < 2 {
                    return None;
                }
                let packed = (slice[0] as u16) | ((slice[1] as u16) << 8);
                let u_dx = ((packed >> 2) & 0x0F) as i8;
                let u_dz = ((packed >> 6) & 0x0F) as i8;
                let u_dy = ((packed >> 10) & 0x07) as i8;
                let u_dyaw = ((packed >> 13) & 0x07) as i8;

                Some((
                    Self::Small {
                        dx: u_dx - 8,
                        dz: u_dz - 8,
                        dy: u_dy - 4,
                        dyaw: u_dyaw - 4,
                    },
                    2,
                ))
            }
            DeltaTier::Medium => {
                if slice.len() < 4 {
                    return None;
                }
                let u_dy = (slice[0] >> 2) & 0x3F;
                let dy = (u_dy as i8) - 32;
                let dx = slice[1] as i8;
                let dz = slice[2] as i8;
                let yaw = QuantizedYaw::from_byte(slice[3]);
                Some((Self::Medium { dx, dz, dy, yaw }, 4))
            }
            DeltaTier::Full => {
                if slice.len() < 8 {
                    return None;
                }
                let mut buf_7b = [0u8; 7];
                buf_7b.copy_from_slice(&slice[1..8]);
                let (coord, yaw, flags) = QuantizedCellCoord::unpack_with_yaw_and_flags(buf_7b);
                Some((Self::Full { coord, yaw, flags }, 8))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delta_tier_stationary_roundtrip() {
        let base_coord = QuantizedCellCoord::new(1000, 500, 2000);
        let base_yaw = QuantizedYaw::from_degrees(45.0);

        let delta = DeltaTransform::compute(base_coord, base_yaw, base_coord, base_yaw, 0);
        assert_eq!(delta, DeltaTransform::Stationary);

        let mut buf = [0u8; 8];
        let bytes_written = delta.pack_to_slice(&mut buf).expect("pack stationary");
        assert_eq!(bytes_written, 1);

        let (unpacked, bytes_read) =
            DeltaTransform::unpack_from_slice(&buf).expect("unpack stationary");
        assert_eq!(bytes_read, 1);
        assert_eq!(unpacked, DeltaTransform::Stationary);

        let (recon_coord, recon_yaw, _) = unpacked.apply(base_coord, base_yaw);
        assert_eq!(recon_coord, base_coord);
        assert_eq!(recon_yaw, base_yaw);
    }

    #[test]
    fn test_delta_tier_small_roundtrip() {
        let base_coord = QuantizedCellCoord::new(1000, 500, 2000);
        let base_yaw = QuantizedYaw::from_byte(100);

        // Displace by +3 steps in X (48 units), -2 steps in Z (-32 units), +1 step in Y (+2 units)
        let target_coord = QuantizedCellCoord::new(1048, 502, 1968);
        let target_yaw = QuantizedYaw::from_byte(base_yaw.as_byte().wrapping_add(8)); // +2 steps of 4

        let delta = DeltaTransform::compute(base_coord, base_yaw, target_coord, target_yaw, 0);
        assert_eq!(delta.tier(), DeltaTier::Small);

        let mut buf = [0u8; 8];
        let bytes_written = delta.pack_to_slice(&mut buf).expect("pack small");
        assert_eq!(bytes_written, 2); // Exactly 2 bytes!

        let (unpacked, bytes_read) = DeltaTransform::unpack_from_slice(&buf).expect("unpack small");
        assert_eq!(bytes_read, 2);
        assert_eq!(unpacked, delta);

        let (recon_coord, recon_yaw, _) = unpacked.apply(base_coord, base_yaw);
        assert_eq!(recon_coord, target_coord);
        assert_eq!(recon_yaw, target_yaw);
    }

    #[test]
    fn test_delta_tier_medium_roundtrip() {
        let base_coord = QuantizedCellCoord::new(1000, 500, 2000);
        let base_yaw = QuantizedYaw::from_degrees(0.0);

        // Sprint displacement: +50 steps in X (400 units), -30 steps in Z (-240 units)
        let target_coord = QuantizedCellCoord::new(1400, 520, 1760);
        let target_yaw = QuantizedYaw::from_degrees(180.0);

        let delta = DeltaTransform::compute(base_coord, base_yaw, target_coord, target_yaw, 0);
        assert_eq!(delta.tier(), DeltaTier::Medium);

        let mut buf = [0u8; 8];
        let bytes_written = delta.pack_to_slice(&mut buf).expect("pack medium");
        assert_eq!(bytes_written, 4); // Exactly 4 bytes!

        let (unpacked, bytes_read) =
            DeltaTransform::unpack_from_slice(&buf).expect("unpack medium");
        assert_eq!(bytes_read, 4);
        assert_eq!(unpacked, delta);

        let (recon_coord, recon_yaw, _) = unpacked.apply(base_coord, base_yaw);
        assert_eq!(recon_coord, target_coord);
        assert_eq!(recon_yaw, target_yaw);
    }

    #[test]
    fn test_delta_tier_full_fallback_on_anchor_flag() {
        let base_coord = QuantizedCellCoord::new(1000, 500, 2000);
        let base_yaw = QuantizedYaw::from_degrees(0.0);

        // Even with zero displacement, cell anchor flag forces Full tier
        let delta =
            DeltaTransform::compute(base_coord, base_yaw, base_coord, base_yaw, FLAG_CELL_ANCHOR);
        assert_eq!(delta.tier(), DeltaTier::Full);

        let mut buf = [0u8; 8];
        let bytes_written = delta.pack_to_slice(&mut buf).expect("pack full");
        assert_eq!(bytes_written, 8);

        let (unpacked, bytes_read) = DeltaTransform::unpack_from_slice(&buf).expect("unpack full");
        assert_eq!(bytes_read, 8);
        assert_eq!(unpacked, delta);
    }
}
