//! Typed client event notifications dispatched to game engines.

/// Typed event emitted by `EidolonClient` to notify the game engine of authoritative state changes.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    /// Successfully established connection and completed cryptographic handshake with server.
    Connected {
        /// Wire protocol version acknowledged by server.
        server_version: u16,
        /// Unique session identifier assigned by server.
        session_id: u64,
    },
    /// Disconnected from server (timeout, server shutdown, or voluntary exit).
    Disconnected {
        /// Explanation for the disconnection.
        reason: &'static str,
    },
    /// An entity entered the local client's Area of Interest (AoI).
    EntitySpawned {
        /// Unique authoritative entity identifier.
        entity_id: u32,
        /// High-level entity classification (0 = player, 1 = monster, 2 = npc, 3 = item).
        entity_type: u8,
        /// Initial continuous world coordinate X.
        x: f32,
        /// Initial continuous world coordinate Y (elevation).
        y: f32,
        /// Initial continuous world coordinate Z.
        z: f32,
        /// Initial facing heading in degrees (0.0 to 360.0).
        yaw_deg: f32,
    },
    /// An entity left the local client's Area of Interest (AoI) or despawned.
    EntityDespawned {
        /// Unique authoritative entity identifier.
        entity_id: u32,
    },
    /// Authoritative state update received for a visible entity.
    EntityUpdated {
        /// Unique authoritative entity identifier.
        entity_id: u32,
        /// Updated continuous world coordinate X.
        x: f32,
        /// Updated continuous world coordinate Y.
        y: f32,
        /// Updated continuous world coordinate Z.
        z: f32,
        /// Updated facing heading in degrees.
        yaw_deg: f32,
        /// Movement intent flags (e.g. walking, sprinting, jumping).
        flags: u8,
    },
    /// Combat event occurred in the vicinity (attack, damage, heal, spell cast).
    CombatAction {
        /// Entity initiating the action.
        source_id: u32,
        /// Entity receiving the action.
        target_id: u32,
        /// Action classification (1 = melee attack, 2 = magic damage, 3 = heal, 4 = death).
        action_type: u8,
        /// Numeric value associated with the action (damage amount, healing amount).
        value: u32,
    },
    /// Item or currency loot acquired and durably persisted in inventory.
    LootAcquired {
        /// Entity receiving the loot.
        entity_id: u32,
        /// Item or currency identifier.
        item_id: u32,
        /// Quantity acquired.
        amount: u32,
    },
    /// Client-side prediction reconciliation event if client position drifts beyond deadband.
    StateReconciled {
        /// Authoritative server position.
        position: [f32; 3],
        /// Authoritative server heading in degrees.
        yaw_deg: f32,
    },
}
