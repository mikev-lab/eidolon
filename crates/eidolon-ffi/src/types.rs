//! C-compatible (`#[repr(C)]`) data representations for cross-language FFI.

/// Standard C-compatible transform representation for game engine position and orientation.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[repr(C)]
pub struct EidolonTransform {
    /// World coordinate X in meters.
    pub x: f32,
    /// World coordinate Y (elevation) in meters.
    pub y: f32,
    /// World coordinate Z in meters.
    pub z: f32,
    /// Facing heading in degrees (0.0 to 360.0).
    pub yaw_degrees: f32,
    /// Velocity along X axis in meters per second.
    pub velocity_x: f32,
    /// Velocity along Z axis in meters per second.
    pub velocity_z: f32,
    /// Movement intent flags (walking, sprinting, jumping).
    pub flags: u8,
    /// Memory alignment padding to 32 bytes.
    pub _padding: [u8; 3],
}

/// Standard C-compatible event notification dispatched to game engine update loops.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[repr(C)]
pub struct EidolonEvent {
    /// Event classification:
    /// - 1: Connected (param1 = session_id, param2 = server_version)
    /// - 2: Disconnected (param1 = reason_code)
    /// - 3: EntitySpawned (entity_id, x, y, z, yaw_degrees, param1 = entity_type)
    /// - 4: EntityDespawned (entity_id)
    /// - 5: EntityUpdated (entity_id, x, y, z, yaw_degrees, param1 = flags)
    /// - 6: CombatAction (source_id = entity_id, param1 = target_id, param2 = (action_type << 32) | value)
    /// - 7: LootAcquired (entity_id, param1 = item_id, param2 = amount)
    pub event_type: u32,
    /// Authoritative entity ID associated with the event.
    pub entity_id: u32,
    /// Continuous coordinate X.
    pub x: f32,
    /// Continuous coordinate Y (elevation).
    pub y: f32,
    /// Continuous coordinate Z.
    pub z: f32,
    /// Facing heading in degrees.
    pub yaw_degrees: f32,
    /// Generic 64-bit parameter 1.
    pub param1: u64,
    /// Generic 64-bit parameter 2.
    pub param2: u64,
}

/// Event type constant: Connected to server.
pub const EIDOLON_EVENT_CONNECTED: u32 = 1;
/// Event type constant: Disconnected from server.
pub const EIDOLON_EVENT_DISCONNECTED: u32 = 2;
/// Event type constant: Entity entered Area of Interest.
pub const EIDOLON_EVENT_ENTITY_SPAWNED: u32 = 3;
/// Event type constant: Entity exited Area of Interest.
pub const EIDOLON_EVENT_ENTITY_DESPAWNED: u32 = 4;
/// Event type constant: Entity transform updated.
pub const EIDOLON_EVENT_ENTITY_UPDATED: u32 = 5;
/// Event type constant: Combat action executed.
pub const EIDOLON_EVENT_COMBAT_ACTION: u32 = 6;
/// Event type constant: Loot acquired.
pub const EIDOLON_EVENT_LOOT_ACQUIRED: u32 = 7;
