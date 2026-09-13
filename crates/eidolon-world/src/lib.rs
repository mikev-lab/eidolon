//! Seamless open-world zone management, ephemeral dungeon instancing, and gacha account hibernation.
//!
//! `eidolon-world` coordinates entity migration across spatial zone borders without loading screens,
//! allocates dynamic dungeon battle rooms, and manages cold-state account snapshots.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod ability;
pub mod building;
pub mod chat;
pub mod chunk_manifest;
pub mod cluster;
pub mod companion;
pub mod durable_journal;
pub mod equipment;
pub mod error;
pub mod hibernation;
pub mod instance;
pub mod interior;
pub mod partition;
pub mod party;
pub mod reconstruction;
pub mod replicated_journal;
pub mod transaction;
pub mod wal;
pub mod zone;

// Re-export primary types for ergonomic crate consumption.
pub use ability::{
    get_ability_definition, AbilityDefinition, AbilityShape, CastInterruptedReason, CastState,
    CooldownTracker,
};
pub use building::{StructureInstance, StructureManager, StructurePiece};
pub use chat::{
    is_within_proximity, ChatChannel, ChatMessage, ChatRateLimiter, DEFAULT_PROXIMITY_RADIUS_METERS,
};
pub use chunk_manifest::{
    ChunkCoord, ChunkManifestManager, StructureChunk, StructureDeltaPacket,
    STRUCTURE_CHUNK_EDGE_METERS,
};
pub use cluster::{
    ClusterTickMetrics, ClusterTopologyConfig, ClusterZoneNode, MultiServerClusterHarness,
    MAX_CLUSTER_NODES, MAX_IN_FLIGHT_MIGRATIONS,
};
pub use companion::{
    AssetManifestDigest, PatchNegotiationState, PatchNegotiator, ZoneAssetRequirement,
};
pub use durable_journal::{CommitDurability, DurableFileJournal, JOURNAL_MAGIC};
pub use equipment::EquipmentContainer;
pub use error::{InstanceId, WorldError, ZoneId};
pub use hibernation::{
    compute_adler32, hydrate_player_into_zone, CharacterRecord, HibernationHeader, PityState,
    PlayerProfile, HIBERNATION_MAGIC, HIBERNATION_SCHEMA_VERSION, MAX_ROSTER_SIZE,
};
pub use instance::{
    DungeonInstance, DungeonPool, InstanceLifecycle, MAX_PARTY_MEMBERS, ROOM_CLEANUP_TIMEOUT_TICKS,
};
pub use interior::{InteriorCell, InteriorCellManager, MAX_INTERIOR_ITEMS_PER_BUILDING};
pub use partition::{
    BoundaryLease, EntityPartitionState, ZonePartitionDetector, ZonePartitionManager,
    DEFAULT_LEASE_DURATION_TICKS, DEFAULT_PARTITION_TIMEOUT_TICKS,
};
pub use party::{Party, PartyManager, PartyMember, MAX_GROUP_MEMBERS};
pub use reconstruction::{
    decode_position_payload, encode_position_payload, reconstruct_zone_from_wal, CheckpointEntity,
    ZoneCheckpoint,
};
pub use replicated_journal::{ReplicatedJournalSink, ReplicationMode};
pub use transaction::{
    AccountState, InventoryItem, TransactionManager, TransactionOp, TransactionStatus,
    LOCK_INVENTORY, LOCK_WALLET, MAX_DEDUP_HISTORY, MAX_INVENTORY_SLOTS,
};
pub use wal::{
    DurableWalSink, MockDurableStorage, WalRecord, WalRingBuffer, WriteAheadJournal,
    DEFAULT_WAL_BUFFER_CAP, MAX_WAL_PAYLOAD_LEN, OP_CHECKPOINT_MARKER, OP_CURRENCY_DELTA,
    OP_ENTITY_DESPAWN, OP_ENTITY_SPAWN, OP_ENTITY_TRANSFORM, OP_INVENTORY_MUTATION, WAL_HEADER_LEN,
};
pub use zone::{MigrationTicket, SeamAxis, WorldManager, WorldZone, ZoneBounds};
