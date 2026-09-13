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
    /// Inter-zone communication link is severed (network partition declared).
    PartitionSevered {
        /// Local zone identifier.
        local_zone: ZoneId,
        /// Remote partitioned zone identifier.
        remote_zone: ZoneId,
    },
    /// Entity is frozen in read-only state due to active network partition.
    BoundaryEntityFrozen(u32),
    /// Boundary cell lease has expired without renewal.
    LeaseExpired {
        /// Expired lease identifier.
        lease_id: u64,
        /// Zone holding expired lease.
        zone_id: ZoneId,
    },
    /// In-memory WAL ring buffer is at full capacity (storage backpressure).
    WalBufferFull,
    /// Durable WAL storage backend is unavailable (outage declared).
    WalStorageUnavailable,
    /// Encoded WAL record byte sequence is corrupted or truncated.
    WalCorruptedRecord(&'static str),
    /// WAL record checksum validation failed.
    WalChecksumMismatch {
        /// Expected checksum from record header.
        expected: u32,
        /// Calculated Adler-32 checksum from payload.
        actual: u32,
    },
    /// Inbound WAL record LSN does not monotonically follow current LSN.
    WalLsnRegression {
        /// Current expected LSN.
        current: u64,
        /// Inbound record LSN.
        incoming: u64,
    },
    /// Atomic transaction failed or rolled back.
    TransactionAborted(&'static str),
    /// Account balance is insufficient for transaction debit.
    InsufficientBalance {
        /// Required balance amount.
        required: u64,
        /// Available account balance.
        actual: u64,
    },
    /// Item identifier not found in character inventory.
    ItemNotFound(u32),
    /// Transaction with this identifier has already been executed.
    DuplicateTransaction(u64),
    /// Stale session attempted to execute inventory mutation without active lock.
    StaleSessionMutation {
        /// Active session holding generation lock.
        expected: u64,
        /// Stale session attempting mutation.
        actual: u64,
    },
    /// Disposable zone crash reconstruction failed.
    ReconstructionFailed(&'static str),
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
            Self::PartitionSevered {
                local_zone,
                remote_zone,
            } => write!(
                f,
                "Network partition severed link between {local_zone} and {remote_zone}"
            ),
            Self::BoundaryEntityFrozen(id) => write!(
                f,
                "Entity {id} is frozen in read-only state due to active partition"
            ),
            Self::LeaseExpired { lease_id, zone_id } => {
                write!(f, "Boundary lease {lease_id} expired for {zone_id}")
            }
            Self::WalBufferFull => write!(f, "WAL in-memory ring buffer is at capacity"),
            Self::WalStorageUnavailable => write!(f, "Durable WAL storage backend is unavailable"),
            Self::WalCorruptedRecord(reason) => write!(f, "Corrupted WAL record: {reason}"),
            Self::WalChecksumMismatch { expected, actual } => {
                write!(
                    f,
                    "WAL record checksum mismatch: expected {expected:#x}, got {actual:#x}"
                )
            }
            Self::WalLsnRegression { current, incoming } => {
                write!(
                    f,
                    "WAL LSN regression: current {current}, incoming {incoming}"
                )
            }
            Self::TransactionAborted(reason) => write!(f, "Transaction aborted: {reason}"),
            Self::InsufficientBalance { required, actual } => {
                write!(
                    f,
                    "Insufficient balance: required {required}, available {actual}"
                )
            }
            Self::ItemNotFound(id) => write!(f, "Item {id} not found in inventory"),
            Self::DuplicateTransaction(id) => write!(f, "Duplicate transaction {id}"),
            Self::StaleSessionMutation { expected, actual } => {
                write!(
                    f,
                    "Stale session mutation rejected: active session {expected}, caller session {actual}"
                )
            }
            Self::ReconstructionFailed(reason) => {
                write!(f, "Zone crash reconstruction failed: {reason}")
            }
        }
    }
}

impl std::error::Error for WorldError {}
