//! Structure chunk manifest hashing, client-side delta caching, and 20-byte mutations.
//!
//! Partitions world structures into 256m x 256m chunks. Each chunk maintains a deterministic
//! 32-bit Adler-32 / CRC manifest hash. When a client's local disk cache matches the server's
//! chunk hash, 0 bytes of structure geometry are transferred over the wire. Mutations
//! are broadcast as compact 20-byte incremental delta packets.

use std::collections::HashMap;

use eidolon_core::fixed::Vec3Fix;

use crate::error::WorldError;
use crate::hibernation::compute_adler32;

/// Edge length of a structure streaming chunk in meters (256.0m x 256.0m).
pub const STRUCTURE_CHUNK_EDGE_METERS: f64 = 256.0;

/// Discrete 2D chunk coordinate identifying a 256m x 256m world cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ChunkCoord {
    /// Horizontal chunk coordinate X.
    pub x: i16,
    /// Horizontal chunk coordinate Z.
    pub z: i16,
}

impl ChunkCoord {
    /// Computes the 256m chunk coordinate containing continuous position `pos`.
    pub fn from_position(pos: Vec3Fix) -> Self {
        let x_m = pos.x.to_f64();
        let z_m = pos.z.to_f64();
        let cx = (x_m / STRUCTURE_CHUNK_EDGE_METERS).floor() as i16;
        let cz = (z_m / STRUCTURE_CHUNK_EDGE_METERS).floor() as i16;
        Self { x: cx, z: cz }
    }
}

/// Compact 20-byte incremental delta packet representing a structure mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct StructureDeltaPacket {
    /// Mutation type: 0 = Placed, 1 = Modified, 2 = Destroyed.
    pub op_type: u8,
    /// Target structure identifier.
    pub structure_id: u32,
    /// Target piece identifier.
    pub piece_id: u32,
    /// Piece classification.
    pub piece_type: u8,
    /// Material and flags.
    pub material_and_flags: u8,
    /// Snapped socket identifier.
    pub snapped_socket: u8,
    /// Local position X in millimeters.
    pub local_x_mm: i16,
    /// Local position Y (elevation) in millimeters.
    pub local_y_mm: i16,
    /// Local position Z in millimeters.
    pub local_z_mm: i16,
    /// Facing heading discrete angle.
    pub local_yaw: u8,
    /// Reserved memory alignment padding.
    pub _padding: u8,
}

impl StructureDeltaPacket {
    /// Mutation opcode: building piece placed.
    pub const OP_PLACED: u8 = 0;
    /// Mutation opcode: building piece modified or repaired.
    pub const OP_MODIFIED: u8 = 1;
    /// Mutation opcode: building piece destroyed or demolished.
    pub const OP_DESTROYED: u8 = 2;

    /// Serializes this mutation packet into an exact 20-byte slice.
    pub fn write_to(&self, out: &mut [u8]) -> Result<(), &'static str> {
        if out.len() < 20 {
            return Err("Buffer too short for StructureDeltaPacket");
        }
        out[0] = self.op_type;
        out[1..5].copy_from_slice(&self.structure_id.to_be_bytes());
        out[5..9].copy_from_slice(&self.piece_id.to_be_bytes());
        out[9] = self.piece_type;
        out[10] = self.material_and_flags;
        out[11] = self.snapped_socket;
        out[12..14].copy_from_slice(&self.local_x_mm.to_be_bytes());
        out[14..16].copy_from_slice(&self.local_y_mm.to_be_bytes());
        out[16..18].copy_from_slice(&self.local_z_mm.to_be_bytes());
        out[18] = self.local_yaw;
        out[19] = self._padding;
        Ok(())
    }

    /// Deserializes a mutation packet from a 20-byte slice.
    pub fn read_from(slice: &[u8]) -> Result<Self, &'static str> {
        if slice.len() < 20 {
            return Err("Slice too short for StructureDeltaPacket");
        }
        let op_type = slice[0];
        let structure_id = u32::from_be_bytes([slice[1], slice[2], slice[3], slice[4]]);
        let piece_id = u32::from_be_bytes([slice[5], slice[6], slice[7], slice[8]]);
        let piece_type = slice[9];
        let material_and_flags = slice[10];
        let snapped_socket = slice[11];
        let local_x_mm = i16::from_be_bytes([slice[12], slice[13]]);
        let local_y_mm = i16::from_be_bytes([slice[14], slice[15]]);
        let local_z_mm = i16::from_be_bytes([slice[16], slice[17]]);
        let local_yaw = slice[18];
        let _padding = slice[19];

        Ok(Self {
            op_type,
            structure_id,
            piece_id,
            piece_type,
            material_and_flags,
            snapped_socket,
            local_x_mm,
            local_y_mm,
            local_z_mm,
            local_yaw,
            _padding,
        })
    }
}

/// Manifest of player structures located within a 256m x 256m world chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureChunk {
    /// Chunk 2D coordinate.
    pub coord: ChunkCoord,
    /// Active structure IDs resident in this chunk.
    pub structure_ids: Vec<u32>,
    /// Adler-32 hash of the chunk contents (structure IDs and versions).
    pub manifest_hash: u32,
    /// Monotonically increasing revision counter.
    pub revision: u32,
}

impl StructureChunk {
    /// Creates a new empty structure chunk at coordinate `coord`.
    pub fn new(coord: ChunkCoord) -> Self {
        Self {
            coord,
            structure_ids: Vec::new(),
            manifest_hash: 0,
            revision: 0,
        }
    }

    /// Recalculates the Adler-32 manifest hash for this chunk.
    pub fn recompute_hash(&mut self) {
        if self.structure_ids.is_empty() {
            self.manifest_hash = 0;
            return;
        }

        // Sort structure IDs for deterministic ordering
        self.structure_ids.sort_unstable();

        let mut hash_payload = Vec::with_capacity(self.structure_ids.len() * 4 + 4);
        hash_payload.extend_from_slice(&self.revision.to_be_bytes());
        for sid in &self.structure_ids {
            hash_payload.extend_from_slice(&sid.to_be_bytes());
        }

        self.manifest_hash = compute_adler32(&hash_payload);
    }
}

/// Manager organizing structure chunks across the world map.
#[derive(Debug, Default)]
pub struct ChunkManifestManager {
    chunks: HashMap<ChunkCoord, StructureChunk>,
}

impl ChunkManifestManager {
    /// Creates a new chunk manifest manager.
    pub fn new() -> Self {
        Self {
            chunks: HashMap::new(),
        }
    }

    /// Registers a structure into the chunk containing `position`.
    pub fn register_structure(&mut self, structure_id: u32, position: Vec3Fix) -> ChunkCoord {
        let coord = ChunkCoord::from_position(position);
        let chunk = self
            .chunks
            .entry(coord)
            .or_insert_with(|| StructureChunk::new(coord));

        if !chunk.structure_ids.contains(&structure_id) {
            chunk.structure_ids.push(structure_id);
            chunk.revision = chunk.revision.wrapping_add(1);
            chunk.recompute_hash();
        }

        coord
    }

    /// Unregisters a structure from its chunk.
    pub fn unregister_structure(
        &mut self,
        structure_id: u32,
        position: Vec3Fix,
    ) -> Result<(), WorldError> {
        let coord = ChunkCoord::from_position(position);
        let chunk = self
            .chunks
            .get_mut(&coord)
            .ok_or(WorldError::StructureNotFound(structure_id))?;

        if let Some(pos) = chunk
            .structure_ids
            .iter()
            .position(|&id| id == structure_id)
        {
            chunk.structure_ids.swap_remove(pos);
            chunk.revision = chunk.revision.wrapping_add(1);
            chunk.recompute_hash();
            Ok(())
        } else {
            Err(WorldError::StructureNotFound(structure_id))
        }
    }

    /// Checks client cache validation.
    ///
    /// Returns true if client's cached hash matches the server's authoritative chunk hash
    /// (triggering 0-byte streaming), or false if a cache mismatch requires streaming.
    pub fn validate_client_cache(&self, coord: ChunkCoord, client_hash: u32) -> bool {
        if let Some(chunk) = self.chunks.get(&coord) {
            chunk.manifest_hash == client_hash
        } else {
            // Empty chunk with hash 0 is considered matching if client claims 0
            client_hash == 0
        }
    }

    /// Returns the active manifest hash for a chunk.
    pub fn get_manifest_hash(&self, coord: ChunkCoord) -> u32 {
        self.chunks
            .get(&coord)
            .map(|c| c.manifest_hash)
            .unwrap_or(0)
    }

    /// Returns the list of structure IDs within a chunk.
    pub fn get_structures_in_chunk(&self, coord: ChunkCoord) -> &[u32] {
        self.chunks
            .get(&coord)
            .map(|c| c.structure_ids.as_slice())
            .unwrap_or(&[])
    }

    /// Marks the chunk containing `position` as modified, bumping revision and recomputing hash.
    pub fn mark_chunk_modified(&mut self, position: Vec3Fix) {
        let coord = ChunkCoord::from_position(position);
        if let Some(chunk) = self.chunks.get_mut(&coord) {
            chunk.revision = chunk.revision.wrapping_add(1);
            chunk.recompute_hash();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_coord_derivation() {
        // (0, 0, 0) -> chunk (0, 0)
        let pos1 = Vec3Fix::ZERO;
        assert_eq!(ChunkCoord::from_position(pos1), ChunkCoord { x: 0, z: 0 });

        // (300, 0, 550) -> chunk (1, 2) since 300 / 256 = 1.17, 550 / 256 = 2.14
        let pos2 = Vec3Fix::from_f64(300.0, 0.0, 550.0);
        assert_eq!(ChunkCoord::from_position(pos2), ChunkCoord { x: 1, z: 2 });

        // (-100, 0, -300) -> chunk (-1, -2)
        let pos3 = Vec3Fix::from_f64(-100.0, 0.0, -300.0);
        assert_eq!(ChunkCoord::from_position(pos3), ChunkCoord { x: -1, z: -2 });
    }

    #[test]
    fn test_structure_delta_packet_roundtrip() {
        let delta = StructureDeltaPacket {
            op_type: StructureDeltaPacket::OP_PLACED,
            structure_id: 1001,
            piece_id: 42,
            piece_type: 1, // Wall
            material_and_flags: 0x12,
            snapped_socket: 3, // East
            local_x_mm: 4000,
            local_y_mm: 0,
            local_z_mm: 0,
            local_yaw: 64,
            _padding: 0,
        };

        let mut buf = [0u8; 20];
        delta.write_to(&mut buf).expect("Write delta packet");

        let decoded = StructureDeltaPacket::read_from(&buf).expect("Read delta packet");
        assert_eq!(decoded, delta);
    }

    #[test]
    fn test_client_cache_validation_zero_byte_streaming() {
        let mut mgr = ChunkManifestManager::new();
        let pos = Vec3Fix::from_f64(100.0, 0.0, 100.0);

        // Register structure 501 in chunk (0, 0)
        let coord = mgr.register_structure(501, pos);
        let server_hash = mgr.get_manifest_hash(coord);
        assert_ne!(server_hash, 0);

        // Client whose cache matches server hash validates as true (0 bytes streamed!)
        assert!(mgr.validate_client_cache(coord, server_hash));

        // Client with stale/outdated cache fails validation (triggers delta streaming)
        assert!(!mgr.validate_client_cache(coord, 0xDEADBEEF));
    }
}
