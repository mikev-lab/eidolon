//! Seamless open-world zone management, ephemeral dungeon instancing, and gacha account hibernation.
//!
//! `eidolon-world` coordinates entity migration across spatial zone borders without loading screens,
//! allocates dynamic dungeon battle rooms, and manages cold-state account snapshots.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod companion;
pub mod error;
pub mod hibernation;
pub mod instance;
pub mod zone;

// Re-export primary types for ergonomic crate consumption.
pub use companion::{
    AssetManifestDigest, PatchNegotiationState, PatchNegotiator, ZoneAssetRequirement,
};
pub use error::{InstanceId, WorldError, ZoneId};
pub use hibernation::{
    compute_adler32, CharacterRecord, HibernationHeader, PityState, PlayerProfile,
    HIBERNATION_MAGIC, HIBERNATION_SCHEMA_VERSION, MAX_ROSTER_SIZE,
};
pub use instance::{
    DungeonInstance, DungeonPool, InstanceLifecycle, MAX_PARTY_MEMBERS, ROOM_CLEANUP_TIMEOUT_TICKS,
};
pub use zone::{MigrationTicket, SeamAxis, WorldManager, WorldZone, ZoneBounds};
