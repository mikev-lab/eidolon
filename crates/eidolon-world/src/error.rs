//! Error types for zone management, dungeon instancing, and account hibernation.

use std::fmt;

/// Unique identifier for an open-world persistent zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ZoneId(pub u32);

impl fmt::Display for ZoneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Zone({})", self.0)
    }
}

/// Unique instance identifier for an ephemeral dungeon room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId(pub u64);

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Instance({})", self.0)
    }
}

/// High-level world, zone, and instance management errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldError {
    /// Requested zone identifier does not exist in the world manager.
    ZoneNotFound(ZoneId),
    /// Requested entity identifier was not found in the zone or instance.
    EntityNotFound(u32),
    /// Entity identifier already exists in the target zone.
    EntityAlreadyExists(u32),
    /// Spatial grid or entity pool in the zone is full.
    ZoneFull(ZoneId),
    /// Requested ephemeral instance identifier was not found in the pool.
    InstanceNotFound(InstanceId),
    /// Ephemeral dungeon instance pool is exhausted.
    InstancePoolExhausted,
    /// Operation is invalid for the current lifecycle state of the instance.
    InvalidInstanceState {
        /// Expected instance state name.
        expected: &'static str,
        /// Actual instance state name.
        actual: &'static str,
    },
    /// Hibernation snapshot buffer is corrupted or has an invalid checksum.
    SnapshotCorrupted,
    /// Hibernation snapshot header or layout is invalid.
    InvalidSnapshot(&'static str),
    /// Party slot is full (maximum 4 co-op players per instance).
    PartyFull,
}

impl fmt::Display for WorldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZoneNotFound(id) => write!(f, "Zone not found: {id}"),
            Self::EntityNotFound(id) => write!(f, "Entity {id} not found in zone"),
            Self::EntityAlreadyExists(id) => write!(f, "Entity {id} already exists in zone"),
            Self::ZoneFull(id) => write!(f, "Zone {id} entity capacity reached"),
            Self::InstanceNotFound(id) => write!(f, "Instance not found: {id}"),
            Self::InstancePoolExhausted => write!(f, "Ephemeral instance pool is exhausted"),
            Self::InvalidInstanceState { expected, actual } => {
                write!(
                    f,
                    "Invalid instance state: expected {expected}, got {actual}"
                )
            }
            Self::SnapshotCorrupted => write!(f, "Hibernation snapshot data is corrupted"),
            Self::InvalidSnapshot(reason) => write!(f, "Invalid hibernation snapshot: {reason}"),
            Self::PartyFull => write!(f, "Co-op party slot limit reached"),
        }
    }
}

impl std::error::Error for WorldError {}
