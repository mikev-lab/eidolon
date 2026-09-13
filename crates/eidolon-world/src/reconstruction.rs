//! Disposable zone server crash reconstruction from periodic checkpoints and durable WAL replay.
//!
//! Treats zone worker nodes as disposable compute. Following a process crash (SIGKILL)
//! or machine drop, replacement nodes deterministically rebuild identical zone state
//! by loading a baseline checkpoint and replaying subsequent WAL mutation records.

use eidolon_core::fixed::{Fixed64, Vec3Fix};

use crate::error::{WorldError, ZoneId};
use crate::wal::{WalRecord, OP_ENTITY_DESPAWN, OP_ENTITY_SPAWN, OP_ENTITY_TRANSFORM};
use crate::zone::{WorldZone, ZoneBounds};

/// Record of an active entity captured inside a periodic zone checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointEntity {
    /// Entity identifier.
    pub entity_id: u32,
    /// Authoritative 3D position.
    pub position: Vec3Fix,
}

/// Periodic snapshot checkpoint of a zone's authoritative state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneCheckpoint {
    /// Zone identifier.
    pub zone_id: ZoneId,
    /// Simulation tick at which the checkpoint was captured.
    pub checkpoint_tick: u64,
    /// Highest WAL Log Sequence Number included in this checkpoint.
    pub last_lsn: u64,
    /// List of entities active in the zone at the moment of checkpointing.
    pub entities: Vec<CheckpointEntity>,
}

impl ZoneCheckpoint {
    /// Captures a checkpoint snapshot from an active zone.
    pub fn capture(
        zone: &WorldZone,
        checkpoint_tick: u64,
        last_lsn: u64,
        active_entity_ids: &[u32],
    ) -> Result<Self, WorldError> {
        let mut entities = Vec::with_capacity(active_entity_ids.len());

        for &eid in active_entity_ids {
            if let Some(pos) = zone.spatial_grid.get_position(eid) {
                entities.push(CheckpointEntity {
                    entity_id: eid,
                    position: pos,
                });
            }
        }

        Ok(Self {
            zone_id: zone.id,
            checkpoint_tick,
            last_lsn,
            entities,
        })
    }
}

/// Encodes a 3D position vector into a 24-byte payload buffer for WAL logging.
pub fn encode_position_payload(pos: Vec3Fix) -> [u8; 24] {
    let mut buf = [0u8; 24];
    buf[0..8].copy_from_slice(&pos.x.raw().to_be_bytes());
    buf[8..16].copy_from_slice(&pos.y.raw().to_be_bytes());
    buf[16..24].copy_from_slice(&pos.z.raw().to_be_bytes());
    buf
}

/// Decodes a 3D position vector from a 24-byte payload buffer.
pub fn decode_position_payload(buf: &[u8]) -> Result<Vec3Fix, WorldError> {
    if buf.len() < 24 {
        return Err(WorldError::WalCorruptedRecord(
            "Position payload buffer too short",
        ));
    }

    let mut x_bytes = [0u8; 8];
    x_bytes.copy_from_slice(&buf[0..8]);
    let x = Fixed64::from_raw(i64::from_be_bytes(x_bytes));

    let mut y_bytes = [0u8; 8];
    y_bytes.copy_from_slice(&buf[8..16]);
    let y = Fixed64::from_raw(i64::from_be_bytes(y_bytes));

    let mut z_bytes = [0u8; 8];
    z_bytes.copy_from_slice(&buf[16..24]);
    let z = Fixed64::from_raw(i64::from_be_bytes(z_bytes));

    Ok(Vec3Fix::new(x, y, z))
}

/// Reconstructs an authoritative `WorldZone` after a server crash by replaying WAL records from a checkpoint.
///
/// Determinism Invariant:
/// - Baseline state is restored from `checkpoint.entities`.
/// - Records with `record.lsn <= checkpoint.last_lsn` are ignored.
/// - Subsequent WAL records are replayed in strict monotonic LSN order.
/// - Generates 100% identical spatial and entity state with zero duplicate entities.
pub fn reconstruct_zone_from_wal(
    checkpoint: &ZoneCheckpoint,
    bounds: ZoneBounds,
    neighbor_zone: Option<ZoneId>,
    neighbor_is_positive: bool,
    max_entities: usize,
    wal_records: &[WalRecord],
) -> Result<WorldZone, WorldError> {
    let mut zone = WorldZone::new(
        checkpoint.zone_id,
        bounds,
        neighbor_zone,
        neighbor_is_positive,
        max_entities,
    );

    // 1. Rehydrate checkpoint baseline entities
    for entity in &checkpoint.entities {
        zone.insert_entity(entity.entity_id, entity.position)?;
    }

    // 2. Filter and replay subsequent WAL records
    let mut expected_lsn = checkpoint.last_lsn;

    for record in wal_records {
        if record.lsn <= checkpoint.last_lsn {
            // Already incorporated into checkpoint snapshot
            continue;
        }

        // Verify strict monotonic LSN progression
        let next_expected = expected_lsn.saturating_add(1);
        if record.lsn != next_expected {
            return Err(WorldError::WalLsnRegression {
                current: next_expected,
                incoming: record.lsn,
            });
        }
        expected_lsn = record.lsn;

        // Apply spatial mutation
        match record.opcode {
            OP_ENTITY_SPAWN => {
                let pos = decode_position_payload(&record.payload[..record.payload_len as usize])?;
                zone.insert_entity(record.entity_id, pos)?;
            }
            OP_ENTITY_DESPAWN => {
                zone.remove_entity(record.entity_id)?;
            }
            OP_ENTITY_TRANSFORM => {
                let pos = decode_position_payload(&record.payload[..record.payload_len as usize])?;
                let _ = zone.update_position(record.entity_id, pos)?;
            }
            _ => {
                // Non-spatial mutations (e.g. currency, inventory) do not alter zone spatial grids
            }
        }
    }

    Ok(zone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zone_crash_reconstruction_parity() {
        let bounds = ZoneBounds::new(
            Fixed64::from_i32(0),
            Fixed64::from_i32(500),
            Fixed64::from_i32(0),
            Fixed64::from_i32(500),
            crate::zone::SeamAxis::EastWest,
            Fixed64::from_i32(484),
            Fixed64::from_i32(500),
        );

        let mut original_zone = WorldZone::new(ZoneId(1), bounds, None, true, 256);

        // Initial entities
        let pos1 = Vec3Fix::new(
            Fixed64::from_i32(50),
            Fixed64::from_i32(0),
            Fixed64::from_i32(50),
        );
        let pos2 = Vec3Fix::new(
            Fixed64::from_i32(100),
            Fixed64::from_i32(0),
            Fixed64::from_i32(100),
        );

        original_zone.insert_entity(101, pos1).unwrap();
        original_zone.insert_entity(102, pos2).unwrap();

        // Checkpoint at tick 50, LSN 10
        let checkpoint = ZoneCheckpoint::capture(&original_zone, 50, 10, &[101, 102]).unwrap();
        assert_eq!(checkpoint.entities.len(), 2);

        // Subsequent WAL mutations:
        // LSN 11: Entity 103 spawns at (150, 0, 150)
        let pos3 = Vec3Fix::new(
            Fixed64::from_i32(150),
            Fixed64::from_i32(0),
            Fixed64::from_i32(150),
        );
        let rec11 = WalRecord::new(
            11,
            51,
            1003,
            103,
            OP_ENTITY_SPAWN,
            &encode_position_payload(pos3),
        )
        .unwrap();

        // LSN 12: Entity 101 moves to (75, 0, 75)
        let pos1_new = Vec3Fix::new(
            Fixed64::from_i32(75),
            Fixed64::from_i32(0),
            Fixed64::from_i32(75),
        );
        let rec12 = WalRecord::new(
            12,
            52,
            1001,
            101,
            OP_ENTITY_TRANSFORM,
            &encode_position_payload(pos1_new),
        )
        .unwrap();

        // LSN 13: Entity 102 despawns
        let rec13 = WalRecord::new(13, 53, 1002, 102, OP_ENTITY_DESPAWN, &[]).unwrap();

        let wal_stream = vec![rec11, rec12, rec13];

        // Simulate server crash and reconstruct on a replacement worker
        let reconstructed =
            reconstruct_zone_from_wal(&checkpoint, bounds, None, true, 256, &wal_stream)
                .expect("Zone reconstruction must succeed");

        // Verify reconstructed state:
        // Entity 101: exists at (75, 0, 75)
        assert_eq!(reconstructed.spatial_grid.get_position(101), Some(pos1_new));

        // Entity 102: despawned (must not exist)
        assert_eq!(reconstructed.spatial_grid.get_position(102), None);

        // Entity 103: exists at (150, 0, 150)
        assert_eq!(reconstructed.spatial_grid.get_position(103), Some(pos3));

        // Total entity count must be exactly 2
        assert_eq!(reconstructed.spatial_grid.active_count(), 2);
    }
}
