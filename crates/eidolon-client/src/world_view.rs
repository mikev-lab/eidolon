//! In-memory client entity registry and 60/120/144 FPS dead reckoning extrapolation.

use std::collections::HashMap;
use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::kinematics::{extrapolate, KinematicState};
use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw};

/// Float32 transform representation suitable for direct consumption by game engines.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ClientTransform {
    /// Global world coordinate X in meters.
    pub x: f32,
    /// Global world coordinate Y (elevation) in meters.
    pub y: f32,
    /// Global world coordinate Z in meters.
    pub z: f32,
    /// Facing heading in degrees (0.0 to 360.0).
    pub yaw_deg: f32,
    /// Extrapolated velocity X in meters per second.
    pub vx: f32,
    /// Extrapolated velocity Z in meters per second.
    pub vz: f32,
    /// Movement intent flags (walking, sprinting, jumping).
    pub flags: u8,
}

/// Authoritative entity state maintained on the client.
#[derive(Debug, Clone)]
pub struct RemoteEntity {
    /// Authoritative entity ID.
    pub entity_id: u32,
    /// Entity classification (player, monster, npc, item).
    pub entity_type: u8,
    /// Grid cell index X.
    pub cell_x: i32,
    /// Grid cell index Y.
    pub cell_y: i32,
    /// Grid cell index Z.
    pub cell_z: i32,
    /// Quantized cell-relative position.
    pub quant_coord: QuantizedCellCoord,
    /// Discrete 1-byte yaw angle.
    pub yaw: QuantizedYaw,
    /// Continuous fixed-point velocity vector in meters/second.
    pub velocity: Vec3Fix,
    /// Movement state flags.
    pub flags: u8,
    /// Timestamp when this entity state was last updated by the server.
    pub last_update: Instant,
    /// Health progression points.
    pub health: u32,
    /// Maximum health capacity.
    pub max_health: u32,
}

impl RemoteEntity {
    /// Calculates the continuous global world position at the moment of the last server update.
    pub fn global_position(&self) -> Vec3Fix {
        QuantizedCellCoord::dequantize_to_global(
            self.cell_x,
            self.cell_y,
            self.cell_z,
            self.quant_coord,
        )
    }

    /// Extrapolates entity transform to `t + delta_seconds` using deterministic dead reckoning.
    pub fn extrapolate_transform(&self, delta_seconds: f32) -> ClientTransform {
        let base_pos = self.global_position();
        let dt = Fixed64::from_f64(delta_seconds.max(0.0) as f64);

        let initial_kinematics =
            KinematicState::with_velocity(base_pos, self.velocity, self.yaw, self.flags);

        let extrapolated = extrapolate(&initial_kinematics, 1, dt);
        let (x, y, z) = extrapolated.position.to_f64();
        let (vx, _vy, vz) = extrapolated.velocity.to_f64();

        ClientTransform {
            x: x as f32,
            y: y as f32,
            z: z as f32,
            yaw_deg: extrapolated.yaw.to_degrees() as f32,
            vx: vx as f32,
            vz: vz as f32,
            flags: extrapolated.flags,
        }
    }
}

/// In-memory entity table managing all visible entities within the client's Area of Interest.
#[derive(Debug, Default)]
pub struct ClientWorldView {
    entities: HashMap<u32, RemoteEntity>,
}

impl ClientWorldView {
    /// Creates a new empty client world view.
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
        }
    }

    /// Registers or updates an entity in the client world view.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_entity(
        &mut self,
        entity_id: u32,
        entity_type: u8,
        cell_x: i32,
        cell_y: i32,
        cell_z: i32,
        quant_coord: QuantizedCellCoord,
        yaw: QuantizedYaw,
        flags: u8,
        velocity: Vec3Fix,
    ) {
        let entity = self
            .entities
            .entry(entity_id)
            .or_insert_with(|| RemoteEntity {
                entity_id,
                entity_type,
                cell_x,
                cell_y,
                cell_z,
                quant_coord,
                yaw,
                velocity,
                flags,
                last_update: Instant::now(),
                health: 100,
                max_health: 100,
            });

        entity.cell_x = cell_x;
        entity.cell_y = cell_y;
        entity.cell_z = cell_z;
        entity.quant_coord = quant_coord;
        entity.yaw = yaw;
        entity.flags = flags;
        entity.velocity = velocity;
        entity.last_update = Instant::now();
    }

    /// Removes an entity from the client world view (e.g. upon AoI exit or despawn).
    pub fn remove_entity(&mut self, entity_id: u32) -> bool {
        self.entities.remove(&entity_id).is_some()
    }

    /// Returns a reference to a specific entity if present.
    pub fn get_entity(&self, entity_id: u32) -> Option<&RemoteEntity> {
        self.entities.get(&entity_id)
    }

    /// Returns a mutable reference to a specific entity.
    pub fn get_entity_mut(&mut self, entity_id: u32) -> Option<&mut RemoteEntity> {
        self.entities.get_mut(&entity_id)
    }

    /// Extrapolates an entity's position to the current rendering frame given `delta_time` in seconds.
    pub fn extrapolate_entity(&self, entity_id: u32, delta_time: f32) -> Option<ClientTransform> {
        self.entities
            .get(&entity_id)
            .map(|e| e.extrapolate_transform(delta_time))
    }

    /// Returns a list of all visible entity IDs currently tracked by the client.
    pub fn get_visible_entities(&self) -> Vec<u32> {
        self.entities.keys().copied().collect()
    }

    /// Returns the total count of visible entities currently tracked.
    pub fn count(&self) -> usize {
        self.entities.len()
    }

    /// Clears all tracked entities from the world view (e.g. upon disconnect or zone teleport).
    pub fn clear(&mut self) {
        self.entities.clear();
    }
}
