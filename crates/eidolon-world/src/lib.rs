//! Seamless open-world zone management, ephemeral dungeon instancing, and gacha account hibernation.
//!
//! `eidolon-world` coordinates entity migration across spatial zone borders without loading screens,
//! allocates dynamic dungeon battle rooms, and manages cold-state account snapshots.

#![deny(unsafe_code)]
#![warn(missing_docs)]

/// Seamless persistent open-world zones and border handoff state machines.
pub mod zone {
    /// Unique identifier for an open-world persistent zone.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct ZoneId(pub u32);

    /// Entity state descriptor during border crossing handoffs.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MigrationTicket {
        /// Unique entity identifier.
        pub entity_id: u64,
        /// Source zone identifier.
        pub from_zone: ZoneId,
        /// Destination zone identifier.
        pub to_zone: ZoneId,
    }
}

/// Ephemeral dungeon instances and co-op raid battle rooms.
pub mod dungeon {
    use crate::zone::ZoneId;

    /// Unique instance identifier for an ephemeral room.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct InstanceId(pub u64);

    /// Lifecycle state of an ephemeral instance room.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum InstanceState {
        /// Room is allocated and initializing assets.
        Initializing,
        /// Room is active with connected players.
        Active,
        /// Room is cleared or abandoned; marked for immediate deallocation.
        PendingCleanup,
    }

    /// Descriptor for a lightweight dungeon battle instance.
    #[derive(Debug)]
    pub struct DungeonRoom {
        /// Room instance identifier.
        pub id: InstanceId,
        /// Base zone blueprint identifier.
        pub zone_id: ZoneId,
        /// Current lifecycle status.
        pub state: InstanceState,
        /// Number of currently connected party members.
        pub player_count: u32,
    }

    impl DungeonRoom {
        /// Creates a new dungeon room in the `Initializing` state.
        pub fn new(id: InstanceId, zone_id: ZoneId) -> Self {
            Self {
                id,
                zone_id,
                state: InstanceState::Initializing,
                player_count: 0,
            }
        }
    }
}

/// Cold-state account hibernation for long-tail live-service games.
pub mod hibernation {
    /// Header identifying a cold-state hibernated player account snapshot.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct HibernationHeader {
        /// Account identifier.
        pub account_id: u64,
        /// Last recorded login timestamp (seconds since UNIX epoch).
        pub last_active_epoch_sec: u64,
        /// Serialized snapshot byte length.
        pub payload_byte_len: u32,
    }
}

#[cfg(test)]
mod tests {
    use super::dungeon::{DungeonRoom, InstanceId, InstanceState};
    use super::zone::ZoneId;

    #[test]
    fn test_dungeon_room_lifecycle() {
        let room = DungeonRoom::new(InstanceId(101), ZoneId(5));
        assert_eq!(room.id, InstanceId(101));
        assert_eq!(room.zone_id, ZoneId(5));
        assert_eq!(room.state, InstanceState::Initializing);
        assert_eq!(room.player_count, 0);
    }
}
