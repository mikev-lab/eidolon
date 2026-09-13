//! Player building structures, socket snapping, load-bearing stability, and interior item primitives.
//!
//! Provides deterministic 3-byte bitpacked building pieces, Valheim-style structural
//! load-bearing decay models, socket snapping graphs, and 14-byte interior item records
//! with zero third-party dependencies.

use crate::fixed::Vec3Fix;
use crate::quant::QuantizedYaw;

/// Maximum stability score for fully grounded foundations.
pub const MAX_STABILITY: u8 = 100;

/// Standard horizontal grid snapping unit in meters (foundation and wall width/length).
pub const STANDARD_GRID_HORIZONTAL_METERS: f64 = 4.0;

/// Standard vertical grid snapping unit in meters (wall and pillar height).
pub const STANDARD_GRID_VERTICAL_METERS: f64 = 3.0;

/// Building piece structural classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PieceType {
    /// Grounded load-bearing foundation slab (4m x 4m x 1m).
    Foundation = 0,
    /// Vertical solid wall panel (4m x 3m x 0.2m).
    Wall = 1,
    /// Wall with integrated doorway frame (4m x 3m x 0.2m).
    DoorFrame = 2,
    /// Wall with integrated window opening (4m x 3m x 0.2m).
    WindowWall = 3,
    /// Horizontal ceiling or upper floor panel (4m x 4m x 0.2m).
    Floor = 4,
    /// Angled roof panel for weather sheltering (4m x 4m).
    Roof = 5,
    /// Diagonal stairs ascending one level (4m length, 3m rise).
    Stairs = 6,
    /// Vertical load-bearing support column (0.4m x 0.4m x 3m).
    Pillar = 7,
}

impl PieceType {
    /// Decodes a piece type from a raw byte value.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Foundation),
            1 => Some(Self::Wall),
            2 => Some(Self::DoorFrame),
            3 => Some(Self::WindowWall),
            4 => Some(Self::Floor),
            5 => Some(Self::Roof),
            6 => Some(Self::Stairs),
            7 => Some(Self::Pillar),
            _ => None,
        }
    }

    /// Returns true if this piece type acts as an inherently grounded foundation.
    pub fn is_foundation(&self) -> bool {
        matches!(self, Self::Foundation)
    }

    /// Returns the approximate Axis-Aligned Bounding Box (AABB) half-extents in meters.
    pub fn half_extents(&self) -> Vec3Fix {
        match self {
            Self::Foundation => Vec3Fix::from_f64(2.0, 0.5, 2.0),
            Self::Wall | Self::DoorFrame | Self::WindowWall => Vec3Fix::from_f64(2.0, 1.5, 0.1),
            Self::Floor => Vec3Fix::from_f64(2.0, 0.1, 2.0),
            Self::Roof => Vec3Fix::from_f64(2.0, 1.0, 2.0),
            Self::Stairs => Vec3Fix::from_f64(2.0, 1.5, 2.0),
            Self::Pillar => Vec3Fix::from_f64(0.2, 1.5, 0.2),
        }
    }
}

/// Construction material tiers governing durability and stability decay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MaterialType {
    /// Basic timber construction (fast to gather, higher stability decay).
    Wood = 0,
    /// Stone masonry (high compressive strength, low decay per vertical tier).
    Stone = 1,
    /// Reinforced iron / metal framework (industrial strength, low decay).
    Metal = 2,
    /// High-tier reinforced composite (maximum durability for fortresses).
    Reinforced = 3,
}

impl MaterialType {
    /// Decodes a material type from a 4-bit nibble.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val & 0x0F {
            0 => Some(Self::Wood),
            1 => Some(Self::Stone),
            2 => Some(Self::Metal),
            3 => Some(Self::Reinforced),
            _ => None,
        }
    }

    /// Maximum baseline hit points for pieces of this material.
    pub fn max_health(&self) -> u32 {
        match self {
            Self::Wood => 250,
            Self::Stone => 500,
            Self::Metal => 1000,
            Self::Reinforced => 2000,
        }
    }

    /// Stability points deducted per structural jump from grounded foundation.
    pub fn stability_decay(&self) -> u8 {
        match self {
            Self::Wood => 15,      // Max ~6 pieces tall/wide
            Self::Stone => 8,      // Max ~12 pieces tall/wide
            Self::Metal => 4,      // Max ~25 pieces tall/wide
            Self::Reinforced => 2, // Max ~50 pieces tall/wide
        }
    }
}

/// Snapping socket identifiers on parent building pieces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SnapSocket {
    /// Snapped directly to center or foundation root.
    Center = 0,
    /// North horizontal edge socket (+Z axis).
    North = 1,
    /// South horizontal edge socket (-Z axis).
    South = 2,
    /// East horizontal edge socket (+X axis).
    East = 3,
    /// West horizontal edge socket (-X axis).
    West = 4,
    /// Top vertical socket (+Y axis, upward stacking).
    Top = 5,
    /// Bottom vertical socket (-Y axis, downward support).
    Bottom = 6,
    /// Diagonal 45-degree corner socket.
    Diagonal45 = 7,
}

impl SnapSocket {
    /// Decodes a snap socket from a byte value.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Center),
            1 => Some(Self::North),
            2 => Some(Self::South),
            3 => Some(Self::East),
            4 => Some(Self::West),
            5 => Some(Self::Top),
            6 => Some(Self::Bottom),
            7 => Some(Self::Diagonal45),
            _ => None,
        }
    }

    /// Returns the local positional offset in meters associated with this socket.
    pub fn local_offset(&self) -> Vec3Fix {
        match self {
            Self::Center => Vec3Fix::ZERO,
            Self::North => Vec3Fix::from_f64(0.0, 0.0, STANDARD_GRID_HORIZONTAL_METERS),
            Self::South => Vec3Fix::from_f64(0.0, 0.0, -STANDARD_GRID_HORIZONTAL_METERS),
            Self::East => Vec3Fix::from_f64(STANDARD_GRID_HORIZONTAL_METERS, 0.0, 0.0),
            Self::West => Vec3Fix::from_f64(-STANDARD_GRID_HORIZONTAL_METERS, 0.0, 0.0),
            Self::Top => Vec3Fix::from_f64(0.0, STANDARD_GRID_VERTICAL_METERS, 0.0),
            Self::Bottom => Vec3Fix::from_f64(0.0, -STANDARD_GRID_VERTICAL_METERS, 0.0),
            Self::Diagonal45 => {
                let diag = STANDARD_GRID_HORIZONTAL_METERS * core::f64::consts::FRAC_1_SQRT_2;
                Vec3Fix::from_f64(diag, 0.0, diag)
            }
        }
    }
}

/// Compact 3-byte bitpacked representation of a freeform building piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StructurePieceData {
    /// Type of building piece.
    pub piece_type: u8,
    /// Material (lower 4 bits) and visual variant flags (upper 4 bits).
    pub material_and_flags: u8,
    /// Snapped socket ID on parent piece.
    pub snapped_socket: u8,
}

impl StructurePieceData {
    /// Creates a new bitpacked structure piece data record.
    pub fn new(
        piece_type: PieceType,
        material: MaterialType,
        socket: SnapSocket,
        variant_flags: u8,
    ) -> Self {
        let mat_nibble = (material as u8) & 0x0F;
        let flags_nibble = (variant_flags & 0x0F) << 4;
        Self {
            piece_type: piece_type as u8,
            material_and_flags: mat_nibble | flags_nibble,
            snapped_socket: socket as u8,
        }
    }

    /// Returns the parsed `PieceType`.
    pub fn piece_type(&self) -> Option<PieceType> {
        PieceType::from_u8(self.piece_type)
    }

    /// Returns the parsed `MaterialType`.
    pub fn material(&self) -> Option<MaterialType> {
        MaterialType::from_u8(self.material_and_flags & 0x0F)
    }

    /// Returns the 4-bit visual variant flags.
    pub fn variant_flags(&self) -> u8 {
        (self.material_and_flags >> 4) & 0x0F
    }

    /// Returns the parsed `SnapSocket`.
    pub fn socket(&self) -> Option<SnapSocket> {
        SnapSocket::from_u8(self.snapped_socket)
    }

    /// Serializes the piece data to an exact 3-byte array.
    pub fn encode(&self) -> [u8; 3] {
        [
            self.piece_type,
            self.material_and_flags,
            self.snapped_socket,
        ]
    }

    /// Deserializes piece data from a 3-byte array.
    pub fn decode(bytes: [u8; 3]) -> Result<Self, &'static str> {
        let piece = Self {
            piece_type: bytes[0],
            material_and_flags: bytes[1],
            snapped_socket: bytes[2],
        };

        if piece.piece_type().is_none() {
            return Err("Invalid PieceType opcode");
        }
        if piece.material().is_none() {
            return Err("Invalid MaterialType nibble");
        }
        if piece.socket().is_none() {
            return Err("Invalid SnapSocket identifier");
        }

        Ok(piece)
    }
}

/// Calculates the structural load-bearing stability of a piece given its parent's stability.
///
/// If `is_grounded` is true (e.g. a foundation directly touching terrain), stability is 100.
/// Otherwise, stability decays by `material.stability_decay()`. Returns 0 if the piece cannot stand.
pub fn calculate_stability(is_grounded: bool, parent_stability: u8, material: MaterialType) -> u8 {
    if is_grounded {
        MAX_STABILITY
    } else {
        parent_stability.saturating_sub(material.stability_decay())
    }
}

/// Compact 14-byte representation of a customized interior item in a player house.
///
/// Designed for high-density housing with up to 3,000 decorative items per building.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InteriorItemTransform {
    /// Unique item instance identifier within the interior cell.
    pub item_instance_id: u32,
    /// Canonical item catalog definition ID (e.g. table, chair, sword rack).
    pub item_type_id: u32,
    /// Local room coordinate X in millimeters (-32,768 mm to +32,767 mm).
    pub local_x_mm: i16,
    /// Local room coordinate Y (elevation) in millimeters (-32,768 mm to +32,767 mm).
    pub local_y_mm: i16,
    /// Local room coordinate Z in millimeters (-32,768 mm to +32,767 mm).
    pub local_z_mm: i16,
    /// Local discrete facing heading (0..255).
    pub local_yaw: QuantizedYaw,
    /// Behavioral flags (interactive, container, light source, locked).
    pub flags: u8,
}

impl InteriorItemTransform {
    /// Flag: item is interactable by players.
    pub const FLAG_INTERACTIVE: u8 = 0x01;
    /// Flag: item is a storage container with inventory slots.
    pub const FLAG_CONTAINER: u8 = 0x02;
    /// Flag: item emits point illumination.
    pub const FLAG_LIGHT_SOURCE: u8 = 0x04;
    /// Flag: item placement is locked by house owner.
    pub const FLAG_LOCKED: u8 = 0x08;

    /// Creates a new interior item transform from continuous floating-point meter coordinates.
    pub fn from_meters(
        item_instance_id: u32,
        item_type_id: u32,
        x_m: f32,
        y_m: f32,
        z_m: f32,
        yaw_deg: f32,
        flags: u8,
    ) -> Self {
        let x_mm = (x_m * 1000.0).clamp(-32768.0, 32767.0) as i16;
        let y_mm = (y_m * 1000.0).clamp(-32768.0, 32767.0) as i16;
        let z_mm = (z_m * 1000.0).clamp(-32768.0, 32767.0) as i16;

        Self {
            item_instance_id,
            item_type_id,
            local_x_mm: x_mm,
            local_y_mm: y_mm,
            local_z_mm: z_mm,
            local_yaw: QuantizedYaw::from_degrees(yaw_deg as f64),
            flags,
        }
    }

    /// Returns continuous local room coordinate X in meters.
    pub fn x_meters(&self) -> f32 {
        self.local_x_mm as f32 / 1000.0
    }

    /// Returns continuous local room coordinate Y (elevation) in meters.
    pub fn y_meters(&self) -> f32 {
        self.local_y_mm as f32 / 1000.0
    }

    /// Returns continuous local room coordinate Z in meters.
    pub fn z_meters(&self) -> f32 {
        self.local_z_mm as f32 / 1000.0
    }

    /// Encodes this record to an exact 14-byte wire buffer.
    pub fn encode(&self) -> [u8; 14] {
        let mut buf = [0u8; 14];
        buf[0..4].copy_from_slice(&self.item_instance_id.to_be_bytes());
        buf[4..8].copy_from_slice(&self.item_type_id.to_be_bytes());
        buf[8..10].copy_from_slice(&self.local_x_mm.to_be_bytes());
        buf[10..12].copy_from_slice(&self.local_y_mm.to_be_bytes());
        buf[12..14].copy_from_slice(&self.local_z_mm.to_be_bytes());
        buf
    }
}

/// Compact 16-byte packed interior item transform for memory alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct InteriorItemRecord {
    /// Unique item instance identifier within the interior cell.
    pub item_instance_id: u32,
    /// Canonical item catalog definition ID.
    pub item_type_id: u32,
    /// Local room coordinate X in millimeters (-32,768 mm to +32,767 mm).
    pub local_x_mm: i16,
    /// Local room coordinate Y (elevation) in millimeters.
    pub local_y_mm: i16,
    /// Local room coordinate Z in millimeters.
    pub local_z_mm: i16,
    /// Local discrete facing heading.
    pub local_yaw: u8,
    /// Behavioral flags.
    pub flags: u8,
}

impl InteriorItemRecord {
    /// Serializes this record into an exact 16-byte buffer.
    pub fn write_to(&self, out: &mut [u8]) -> Result<(), &'static str> {
        if out.len() < 16 {
            return Err("Buffer too short for InteriorItemRecord");
        }
        out[0..4].copy_from_slice(&self.item_instance_id.to_be_bytes());
        out[4..8].copy_from_slice(&self.item_type_id.to_be_bytes());
        out[8..10].copy_from_slice(&self.local_x_mm.to_be_bytes());
        out[10..12].copy_from_slice(&self.local_y_mm.to_be_bytes());
        out[12..14].copy_from_slice(&self.local_z_mm.to_be_bytes());
        out[14] = self.local_yaw;
        out[15] = self.flags;
        Ok(())
    }

    /// Deserializes a record from a 16-byte slice.
    pub fn read_from(slice: &[u8]) -> Result<Self, &'static str> {
        if slice.len() < 16 {
            return Err("Slice too short for InteriorItemRecord");
        }
        let item_instance_id = u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]);
        let item_type_id = u32::from_be_bytes([slice[4], slice[5], slice[6], slice[7]]);
        let local_x_mm = i16::from_be_bytes([slice[8], slice[9]]);
        let local_y_mm = i16::from_be_bytes([slice[10], slice[11]]);
        let local_z_mm = i16::from_be_bytes([slice[12], slice[13]]);
        let local_yaw = slice[14];
        let flags = slice[15];

        Ok(Self {
            item_instance_id,
            item_type_id,
            local_x_mm,
            local_y_mm,
            local_z_mm,
            local_yaw,
            flags,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_structure_piece_data_bitpacking_roundtrip() {
        let piece = StructurePieceData::new(
            PieceType::DoorFrame,
            MaterialType::Stone,
            SnapSocket::North,
            0x0B,
        );

        let bytes = piece.encode();
        let decoded = StructurePieceData::decode(bytes).expect("Decode piece data");

        assert_eq!(decoded.piece_type(), Some(PieceType::DoorFrame));
        assert_eq!(decoded.material(), Some(MaterialType::Stone));
        assert_eq!(decoded.socket(), Some(SnapSocket::North));
        assert_eq!(decoded.variant_flags(), 0x0B);
    }

    #[test]
    fn test_stability_propagation_and_decay() {
        // Grounded stone foundation has max stability
        let s0 = calculate_stability(true, 0, MaterialType::Stone);
        assert_eq!(s0, 100);

        // First wall layer above foundation
        let s1 = calculate_stability(false, s0, MaterialType::Stone);
        assert_eq!(s1, 92); // 100 - 8

        // Second wall layer
        let s2 = calculate_stability(false, s1, MaterialType::Stone);
        assert_eq!(s2, 84); // 92 - 8

        // Wood decays faster (15 points per jump)
        let w0 = calculate_stability(true, 0, MaterialType::Wood);
        assert_eq!(w0, 100);
        let w1 = calculate_stability(false, w0, MaterialType::Wood);
        assert_eq!(w1, 85);

        // Stability saturates at 0 and does not underflow
        let mut curr = 10u8;
        curr = calculate_stability(false, curr, MaterialType::Wood);
        assert_eq!(curr, 0);
    }

    #[test]
    fn test_interior_item_record_serialization() {
        let record = InteriorItemRecord {
            item_instance_id: 9901,
            item_type_id: 42,
            local_x_mm: 1500,  // 1.5m
            local_y_mm: 750,   // 0.75m
            local_z_mm: -2000, // -2.0m
            local_yaw: 128,
            flags: InteriorItemTransform::FLAG_INTERACTIVE | InteriorItemTransform::FLAG_CONTAINER,
        };

        let mut buf = [0u8; 16];
        record.write_to(&mut buf).expect("Write to buffer");

        let decoded = InteriorItemRecord::read_from(&buf).expect("Read from buffer");
        assert_eq!(decoded, record);
    }
}
