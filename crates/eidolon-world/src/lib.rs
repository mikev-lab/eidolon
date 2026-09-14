//! Seamless open-world zone management, ephemeral dungeon instancing, and gacha account hibernation.
//!
//! `eidolon-world` coordinates entity migration across spatial zone borders without loading screens,
//! allocates dynamic dungeon battle rooms, and manages cold-state account snapshots.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod ability;
pub mod adaptive_shard;
pub mod building;
pub mod chat;
pub mod chunk_manifest;
pub mod cluster;
pub mod companion;
pub mod durable_journal;
pub mod equipment;
pub mod error;
pub mod flight_recorder;
pub mod flocking;
pub mod hibernation;
pub mod instance;
pub mod interior;
pub mod item_transaction;
pub mod market_coordinator;
pub mod npc_ecosystem;
pub mod partition;
pub mod party;
pub mod predictive_migration;
pub mod reconstruction;
pub mod replicated_journal;
pub mod script_engine;
pub mod sector_parallel;
pub mod soa_storage;
pub mod social;
pub mod territory;
pub mod threat;
pub mod transaction;
pub mod voice_router;
pub mod wal;
pub mod zone;

// Re-export primary types for ergonomic crate consumption.
pub use ability::{
    get_ability_definition, AbilityDefinition, AbilityShape, CastInterruptedReason, CastState,
    CooldownTracker,
};
pub use adaptive_shard::{
    AdaptiveShardManager, RebalanceReason, SectorMetadata, ShardMetrics, ShardRebalanceAction,
    DEFAULT_CPU_OVERLOAD_MICROS, DEFAULT_HOTSPOT_ENTITY_THRESHOLD,
    DEFAULT_WILDERNESS_ENTITY_THRESHOLD, MAX_WORKER_SHARDS,
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
pub use flight_recorder::{
    FlightRecorder, ReplayError, ReplaySession, MAX_INPUT_FRAMES_CAPACITY, MAX_KEYFRAMES_CAPACITY,
};
pub use flocking::compute_separation;
pub use hibernation::{
    compute_adler32, hydrate_player_into_zone, CharacterRecord, HibernationHeader, PityState,
    PlayerProfile, HIBERNATION_MAGIC, HIBERNATION_SCHEMA_VERSION, MAX_ROSTER_SIZE,
};
pub use instance::{
    DungeonInstance, DungeonPool, InstanceLifecycle, MAX_PARTY_MEMBERS, ROOM_CLEANUP_TIMEOUT_TICKS,
};
pub use interior::{InteriorCell, InteriorCellManager, MAX_INTERIOR_ITEMS_PER_BUILDING};
pub use item_transaction::{
    ItemTransactionCoordinator, ItemTransactionError, ItemTransferIntent, ItemTransferResult,
    MAX_TX_DEDUP_CAPACITY,
};
pub use market_coordinator::{
    EscrowError, EscrowHolding, MarketEscrowCoordinator, MarketStation, MAX_ESCROW_ORDERS,
};
pub use npc_ecosystem::{NpcEcosystemManager, NpcEntity, NpcLodTier, NpcState};
pub use partition::{
    BoundaryLease, EntityPartitionState, ZonePartitionDetector, ZonePartitionManager,
    DEFAULT_LEASE_DURATION_TICKS, DEFAULT_PARTITION_TIMEOUT_TICKS,
};
pub use party::{Party, PartyManager, PartyMember, MAX_GROUP_MEMBERS};
pub use predictive_migration::{
    PredictiveMigrationPreAuth, PredictiveSeamPredictor, DEFAULT_LOOKAHEAD_MILLIS,
    DEFAULT_PRE_AUTH_TTL_TICKS, MAX_ACTIVE_PRE_AUTHS,
};
pub use reconstruction::{
    decode_position_payload, encode_position_payload, reconstruct_zone_from_wal, CheckpointEntity,
    ZoneCheckpoint,
};
pub use replicated_journal::{ReplicatedJournalSink, ReplicationMode};
pub use script_engine::{
    HostEnvironment, MockHostEnvironment, OpCode, QuestStatus, ScriptVm, VmError,
    DEFAULT_GAS_LIMIT, VM_STACK_CAPACITY, VM_VARS_CAPACITY,
};
pub use sector_parallel::{
    SectorBucket, SectorCoord, SectorJob, SectorMigration, SectorParallelCoordinator,
    SectorParallelError,
};
pub use soa_storage::{ColdEntityMetadata, SoaEntityStorage, SPARSE_SENTINEL};
pub use social::{
    Alliance, DiplomaticStatus, Guild, GuildMember, SocialError, SocialManager,
    GUILD_PERM_CLAIM_TERRITORY, GUILD_PERM_DEMOTE, GUILD_PERM_EDIT_RANKS, GUILD_PERM_INVITE,
    GUILD_PERM_KICK, GUILD_PERM_MANAGE_DIPLOMACY, GUILD_PERM_PROMOTE, GUILD_PERM_VAULT_DEPOSIT,
    GUILD_PERM_VAULT_WITHDRAW, MAX_GUILD_MEMBERS,
};
pub use territory::{
    TerritoryError, TerritoryManager, TerritoryPlot, TERRITORY_PERM_ACCESS_CONTAINERS,
    TERRITORY_PERM_BUILD, TERRITORY_PERM_ENTER, TERRITORY_PERM_HARVEST, TERRITORY_PERM_PVP_GUARD,
    TERRITORY_PERM_USE_PORTALS,
};
pub use threat::{ThreatEntry, ThreatTable};
pub use transaction::{
    AccountState, InventoryItem, TransactionManager, TransactionOp, TransactionStatus,
    LOCK_INVENTORY, LOCK_WALLET, MAX_DEDUP_HISTORY, MAX_INVENTORY_SLOTS,
};
pub use voice_router::{
    AcousticMaterial, AcousticOcclusionResult, AttenuationModel, SpatialVoiceManager, VoiceEmitter,
    VoiceError, VoiceListener, VoicePeerDescriptor, VoiceRoutingUpdate,
    AIR_ABSORPTION_DB_PER_METER, DEFAULT_MAX_AUDIBLE_DISTANCE, DEFAULT_MIN_AUDIBLE_DISTANCE,
    INAUDIBLE_GAIN_THRESHOLD, MAX_EMITTERS, MAX_LISTENERS, MAX_VOICE_CHANNELS_PER_LISTENER,
};
pub use wal::{
    DurableWalSink, MockDurableStorage, WalRecord, WalRingBuffer, WriteAheadJournal,
    DEFAULT_WAL_BUFFER_CAP, MAX_WAL_PAYLOAD_LEN, OP_CHECKPOINT_MARKER, OP_CURRENCY_DELTA,
    OP_ENTITY_DESPAWN, OP_ENTITY_SPAWN, OP_ENTITY_TRANSFORM, OP_INVENTORY_MUTATION, WAL_HEADER_LEN,
};
pub use zone::{MigrationTicket, SeamAxis, WorldManager, WorldZone, ZoneBounds};
