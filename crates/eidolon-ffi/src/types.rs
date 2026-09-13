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
/// Event type constant: Ability cast started with cast bar duration.
pub const EIDOLON_EVENT_CAST_STARTED: u32 = 8;
/// Event type constant: Active cast interrupted (movement, damage).
pub const EIDOLON_EVENT_CAST_INTERRUPTED: u32 = 9;
/// Event type constant: Ability cast completed successfully.
pub const EIDOLON_EVENT_CAST_COMPLETED: u32 = 10;
/// Event type constant: Chat message received from proximity, party, or shout.
pub const EIDOLON_EVENT_CHAT_MESSAGE: u32 = 11;
/// Event type constant: Equipment slot item updated.
pub const EIDOLON_EVENT_EQUIPMENT_CHANGED: u32 = 12;
/// Event type constant: Party membership or vitals updated.
pub const EIDOLON_EVENT_PARTY_UPDATED: u32 = 13;
/// Event type constant: Time dilation (TiDi) factor changed by server (param1 = fixed-point Q32.32 factor).
pub const EIDOLON_EVENT_TIME_DILATION_CHANGED: u32 = 14;
/// Event type constant: Player structure placed or piece snapped.
pub const EIDOLON_EVENT_STRUCTURE_PLACED: u32 = 15;
/// Event type constant: Player structure or piece destroyed / collapsed.
pub const EIDOLON_EVENT_STRUCTURE_DESTROYED: u32 = 16;
/// Event type constant: Local player entered an interior cell pocket dimension.
pub const EIDOLON_EVENT_INTERIOR_ENTERED: u32 = 17;
/// Event type constant: Local player exited an interior cell back to open-world space.
pub const EIDOLON_EVENT_INTERIOR_EXITED: u32 = 18;
/// Event type constant: Decorative interior item placed or modified.
pub const EIDOLON_EVENT_INTERIOR_ITEM_UPDATED: u32 = 19;
/// Event type constant: Predictive boundary migration pre-authorization token issued.
pub const EIDOLON_EVENT_PREDICTIVE_MIGRATION: u32 = 20;
/// Event type constant: Continental shard boundary rebalanced.
pub const EIDOLON_EVENT_SHARD_REBALANCED: u32 = 21;
/// Event type constant: Macro HLOD terrain chunk loaded.
pub const EIDOLON_EVENT_TERRAIN_CHUNK_LOADED: u32 = 22;

/// Density profile constant: Ultra-low bandwidth mobile / constrained network (1.2 KB/s budget).
pub const EIDOLON_DENSITY_BUDGET_MOBILE: u32 = 0;
/// Density profile constant: Balanced standard MMO experience (2.5 KB/s budget).
pub const EIDOLON_DENSITY_STANDARD_MMO: u32 = 1;
/// Density profile constant: Massive fleet battle or siege mode (4.5 KB/s budget).
pub const EIDOLON_DENSITY_MASSIVE_FLEET_OR_SIEGE: u32 = 2;

/// Standard C-compatible structure piece representation for cross-language game engines.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[repr(C)]
pub struct EidolonStructurePiece {
    /// Authoritative piece identifier.
    pub piece_id: u32,
    /// Parent piece identifier this piece is snapped to (0 if foundation).
    pub parent_piece_id: u32,
    /// Piece classification (0 = Foundation, 1 = Wall, 2 = Floor, 3 = Roof, 4 = Pillar, 5 = Door).
    pub piece_type: u8,
    /// Material tier (0 = Wood, 1 = Stone, 2 = Reinforced, 3 = Metal).
    pub material: u8,
    /// Snapped socket identifier.
    pub socket: u8,
    /// Structural load-bearing stability score (0..100).
    pub stability: u8,
    /// World position X in meters.
    pub x: f32,
    /// World position Y (elevation) in meters.
    pub y: f32,
    /// World position Z in meters.
    pub z: f32,
    /// Facing heading discrete angle in degrees.
    pub yaw_degrees: f32,
    /// Current durability hit points.
    pub health: u32,
    /// Maximum durability hit points.
    pub max_health: u32,
}

/// Standard C-compatible decorative interior item representation for cross-language game engines.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[repr(C)]
pub struct EidolonInteriorItem {
    /// Unique item instance identifier within the interior cell.
    pub item_instance_id: u32,
    /// Catalog template identifier.
    pub item_type_id: u32,
    /// Cell-local coordinate X in meters.
    pub local_x: f32,
    /// Cell-local coordinate Y (elevation) in meters.
    pub local_y: f32,
    /// Cell-local coordinate Z in meters.
    pub local_z: f32,
    /// Cell-local facing heading in degrees.
    pub local_yaw_degrees: f32,
    /// Placement and interaction flags.
    pub flags: u8,
    /// Memory alignment padding.
    pub _padding: [u8; 3],
}

/// Standard C-compatible hierarchical global coordinate for continental and planetary scale.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[repr(C)]
pub struct EidolonGlobalCoord {
    /// Discrete 256m sector index along X axis.
    pub sector_x: i32,
    /// Discrete 256m sector index along Z axis.
    pub sector_z: i32,
    /// Sector-local continuous offset X in meters (0.0 <= offset_x < 256.0).
    pub offset_x: f32,
    /// Elevation coordinate Y in meters.
    pub offset_y: f32,
    /// Sector-local continuous offset Z in meters (0.0 <= offset_z < 256.0).
    pub offset_z: f32,
}

/// Standard C-compatible vehicle kinematic input descriptor for high-speed movement.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[repr(C)]
pub struct EidolonVehicleIntent {
    /// Vehicle classification (0 = GroundSpeeder, 1 = SwoopBike, 2 = Supercar, 3 = Aircraft, 4 = FlyingMount).
    pub vehicle_type: u8,
    /// Throttle percentage input (0 to 100).
    pub throttle: u8,
    /// Steering input (-100 = full left, +100 = full right).
    pub steering: i8,
    /// Pitch angle (-90 to +90 degrees).
    pub pitch: i8,
    /// Roll banking angle (-90 to +90 degrees).
    pub roll: i8,
    /// Memory alignment padding to 8 bytes.
    pub _padding: [u8; 3],
}
