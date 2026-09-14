//! In-memory client entity registry and 60/120/144 FPS dead reckoning extrapolation.

use std::collections::HashMap;
use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::kinematics::{extrapolate, KinematicState};
use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw};

/// Duration in seconds over which new server updates are Hermite-smoothed (50ms = 1 server tick).
pub const HERMITE_SMOOTHING_WINDOW_SECS: f32 = 0.05;

/// Displacement distance in meters exceeding which smoothing is bypassed (teleport or respawn).
pub const SEAM_SNAP_DISTANCE_THRESHOLD: f64 = 10.0;

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
    /// Previous continuous global position before the last server state update.
    pub prev_position: Option<Vec3Fix>,
    /// Previous continuous velocity before the last server state update.
    pub prev_velocity: Option<Vec3Fix>,
    /// Previous discrete yaw before the last server state update.
    pub prev_yaw: Option<QuantizedYaw>,
    /// Timestamp of the server state update prior to the latest update.
    pub prev_update: Option<Instant>,
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

    /// Extrapolates entity transform to `t + delta_seconds` using continuous Hermite smoothing
    /// across cell/zone seam updates, preserving velocity momentum.
    pub fn extrapolate_transform(&self, delta_seconds: f32) -> ClientTransform {
        let base_pos = self.global_position();

        let (render_x, render_y, render_z, render_yaw, render_vx, render_vz) =
            if let (Some(prev_pos), Some(prev_vel), Some(prev_yaw)) =
                (self.prev_position, self.prev_velocity, self.prev_yaw)
            {
                let (p0_x, p0_y, p0_z) = prev_pos.to_f64();
                let (p1_x, p1_y, p1_z) = base_pos.to_f64();
                let dx = p1_x - p0_x;
                let dy = p1_y - p0_y;
                let dz = p1_z - p0_z;
                let dist_sq = dx * dx + dy * dy + dz * dz;

                if dist_sq < SEAM_SNAP_DISTANCE_THRESHOLD * SEAM_SNAP_DISTANCE_THRESHOLD
                    && (0.0..HERMITE_SMOOTHING_WINDOW_SECS).contains(&delta_seconds)
                {
                    let t = (delta_seconds / HERMITE_SMOOTHING_WINDOW_SECS).clamp(0.0, 1.0) as f64;
                    let t2 = t * t;
                    let t3 = t2 * t;

                    // Cubic Hermite basis functions:
                    // h00(t) = 2t^3 - 3t^2 + 1
                    // h10(t) = t^3 - 2t^2 + t
                    // h01(t) = -2t^3 + 3t^2
                    // h11(t) = t^3 - t^2
                    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
                    let h10 = t3 - 2.0 * t2 + t;
                    let h01 = -2.0 * t3 + 3.0 * t2;
                    let h11 = t3 - t2;

                    let (v0_x, v0_y, v0_z) = prev_vel.to_f64();
                    let (v1_x, v1_y, v1_z) = self.velocity.to_f64();
                    let tau = HERMITE_SMOOTHING_WINDOW_SECS as f64;

                    let x = h00 * p0_x + h10 * (tau * v0_x) + h01 * p1_x + h11 * (tau * v1_x);
                    let y = h00 * p0_y + h10 * (tau * v0_y) + h01 * p1_y + h11 * (tau * v1_y);
                    let z = h00 * p0_z + h10 * (tau * v0_z) + h01 * p1_z + h11 * (tau * v1_z);

                    // Derivative: velocity momentum continuity
                    let dh00 = 6.0 * t2 - 6.0 * t;
                    let dh10 = 3.0 * t2 - 4.0 * t + 1.0;
                    let dh01 = -6.0 * t2 + 6.0 * t;
                    let dh11 = 3.0 * t2 - 2.0 * t;

                    let vx =
                        (dh00 * p0_x + dh10 * (tau * v0_x) + dh01 * p1_x + dh11 * (tau * v1_x))
                            / tau;
                    let vz =
                        (dh00 * p0_z + dh10 * (tau * v0_z) + dh01 * p1_z + dh11 * (tau * v1_z))
                            / tau;

                    // Shortest-arc smoothstep angle interpolation
                    let yaw0 = prev_yaw.to_degrees();
                    let yaw1 = self.yaw.to_degrees();
                    let mut diff = (yaw1 - yaw0) % 360.0;
                    if diff > 180.0 {
                        diff -= 360.0;
                    } else if diff < -180.0 {
                        diff += 360.0;
                    }
                    let smooth_yaw =
                        ((yaw0 + diff * (3.0 * t2 - 2.0 * t3)) % 360.0 + 360.0) % 360.0;

                    (x, y, z, smooth_yaw, vx, vz)
                } else {
                    // Beyond smoothing window: extrapolate forward from authoritative target
                    let extra_dt = Fixed64::from_f64(
                        (delta_seconds - HERMITE_SMOOTHING_WINDOW_SECS).max(0.0) as f64,
                    );
                    let initial_kinematics = KinematicState::with_velocity(
                        base_pos,
                        self.velocity,
                        self.yaw,
                        self.flags,
                    );
                    let extrapolated = extrapolate(&initial_kinematics, 1, extra_dt);
                    let (x, y, z) = extrapolated.position.to_f64();
                    let (vx, _vy, vz) = extrapolated.velocity.to_f64();
                    (x, y, z, extrapolated.yaw.to_degrees() as f64, vx, vz)
                }
            } else {
                // Initial update / no prior history: deterministic dead reckoning
                let dt = Fixed64::from_f64(delta_seconds.max(0.0) as f64);
                let initial_kinematics =
                    KinematicState::with_velocity(base_pos, self.velocity, self.yaw, self.flags);
                let extrapolated = extrapolate(&initial_kinematics, 1, dt);
                let (x, y, z) = extrapolated.position.to_f64();
                let (vx, _vy, vz) = extrapolated.velocity.to_f64();
                (x, y, z, extrapolated.yaw.to_degrees() as f64, vx, vz)
            };

        ClientTransform {
            x: render_x as f32,
            y: render_y as f32,
            z: render_z as f32,
            yaw_deg: render_yaw as f32,
            vx: render_vx as f32,
            vz: render_vz as f32,
            flags: self.flags,
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

    /// Registers or updates an entity in the client world view with an explicit timestamp.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_entity_with_time(
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
        timestamp: Instant,
    ) {
        let entry = self.entities.entry(entity_id);
        match entry {
            std::collections::hash_map::Entry::Occupied(mut occ) => {
                let entity = occ.get_mut();
                // Preserve previous state for Hermite smoothing and momentum preservation
                let prev_pos = entity.global_position();
                entity.prev_position = Some(prev_pos);
                entity.prev_velocity = Some(entity.velocity);
                entity.prev_yaw = Some(entity.yaw);
                entity.prev_update = Some(entity.last_update);

                entity.entity_type = entity_type;
                entity.cell_x = cell_x;
                entity.cell_y = cell_y;
                entity.cell_z = cell_z;
                entity.quant_coord = quant_coord;
                entity.yaw = yaw;
                entity.flags = flags;
                entity.velocity = velocity;
                entity.last_update = timestamp;
            }
            std::collections::hash_map::Entry::Vacant(vac) => {
                vac.insert(RemoteEntity {
                    entity_id,
                    entity_type,
                    cell_x,
                    cell_y,
                    cell_z,
                    quant_coord,
                    yaw,
                    velocity,
                    flags,
                    last_update: timestamp,
                    prev_position: None,
                    prev_velocity: None,
                    prev_yaw: None,
                    prev_update: None,
                    health: 100,
                    max_health: 100,
                });
            }
        }
    }

    /// Registers or updates an entity in the client world view using current system time.
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
        self.upsert_entity_with_time(
            entity_id,
            entity_type,
            cell_x,
            cell_y,
            cell_z,
            quant_coord,
            yaw,
            flags,
            velocity,
            Instant::now(),
        );
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

    /// Copies visible entity IDs into `out` slice without dynamic allocation.
    ///
    /// Returns the number of entity IDs written to `out`.
    pub fn get_visible_entities_into(&self, out: &mut [u32]) -> usize {
        let mut count = 0;
        for &id in self.entities.keys() {
            if count >= out.len() {
                break;
            }
            out[count] = id;
            count += 1;
        }
        count
    }

    /// Returns a list of all visible entity IDs currently tracked by the client.
    pub fn get_visible_entities(&self) -> Vec<u32> {
        self.entities.keys().copied().collect()
    }

    /// Returns the total count of visible entities currently tracked.
    pub fn count(&self) -> usize {
        self.entities.len()
    }

    /// Returns an iterator over all tracked remote entities without allocation.
    pub fn iter(&self) -> impl Iterator<Item = &RemoteEntity> {
        self.entities.values()
    }

    /// Returns a mutable iterator over all tracked remote entities without allocation.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut RemoteEntity> {
        self.entities.values_mut()
    }

    /// Clears all tracked entities from the world view (e.g. upon disconnect or zone teleport).
    pub fn clear(&mut self) {
        self.entities.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_world_view_upsert_and_visible_entities_into() {
        let mut wv = ClientWorldView::new();
        assert_eq!(wv.count(), 0);

        wv.upsert_entity(
            10,
            1,
            0,
            0,
            0,
            QuantizedCellCoord::new(0, 0, 0),
            QuantizedYaw::from_degrees(0.0),
            0,
            Vec3Fix::ZERO,
        );
        wv.upsert_entity(
            20,
            1,
            0,
            0,
            0,
            QuantizedCellCoord::new(100, 0, 100),
            QuantizedYaw::from_degrees(90.0),
            0,
            Vec3Fix::ZERO,
        );

        assert_eq!(wv.count(), 2);

        let mut buf = [0u32; 8];
        let written = wv.get_visible_entities_into(&mut buf);
        assert_eq!(written, 2);
        assert!(buf[..2].contains(&10));
        assert!(buf[..2].contains(&20));

        let mut small_buf = [0u32; 1];
        let written_small = wv.get_visible_entities_into(&mut small_buf);
        assert_eq!(written_small, 1);
    }

    #[test]
    fn test_hermite_smoothing_seam_crossing_momentum() {
        let mut wv = ClientWorldView::new();
        let now = Instant::now();

        // Initial spawn at global X = 63.75m (near cell 0 boundary, cell size = 64m) moving East at 10 m/s
        let p0 = Vec3Fix::from_f64(63.75, 0.0, 0.0);
        let (c_x0, c_y0, c_z0, q0) = QuantizedCellCoord::quantize_from_global(p0);

        wv.upsert_entity_with_time(
            42,
            0,
            c_x0,
            c_y0,
            c_z0,
            q0,
            QuantizedYaw::EAST,
            0,
            Vec3Fix::from_f64(10.0, 0.0, 0.0),
            now,
        );

        let entity = wv.get_entity(42).expect("entity exists");
        assert!(entity.prev_position.is_none());

        // Update 50ms later crossing into cell 1 at global X = 64.25m (0.5m displacement, crossing seam)
        let later = now + std::time::Duration::from_millis(50);
        let p1 = Vec3Fix::from_f64(64.25, 0.0, 0.0);
        let (c_x1, c_y1, c_z1, q1) = QuantizedCellCoord::quantize_from_global(p1);
        assert_eq!(c_x0, 0);
        assert_eq!(c_x1, 1);

        wv.upsert_entity_with_time(
            42,
            0,
            c_x1,
            c_y1,
            c_z1,
            q1,
            QuantizedYaw::EAST,
            0,
            Vec3Fix::from_f64(12.0, 0.0, 0.0),
            later,
        );

        let entity = wv.get_entity(42).expect("entity exists");
        assert!(entity.prev_position.is_some());

        // Evaluate Hermite smoothing at t=0 of the new update (boundary)
        let t0 = entity.extrapolate_transform(0.0);
        let (p0_x, _, _) = entity.prev_position.unwrap().to_f64();
        // At t=0, position matches previous position exactly
        assert!((t0.x - p0_x as f32).abs() < 1e-3);
        // At t=0, velocity matches previous velocity (10 m/s)
        assert!((t0.vx - 10.0).abs() < 1e-2);

        // Evaluate at midpoint t = 25ms (halfway through smoothing window)
        let t_mid = entity.extrapolate_transform(0.025);
        // Midpoint velocity should be smoothly continuous
        assert!(
            t_mid.vx > 8.0 && t_mid.vx < 14.0,
            "t_mid.vx was {}",
            t_mid.vx
        );

        // Evaluate at completion of smoothing window (t = 50ms = tau)
        let t_end = entity.extrapolate_transform(HERMITE_SMOOTHING_WINDOW_SECS);
        let (p1_x, _, _) = entity.global_position().to_f64();
        assert!((t_end.x - p1_x as f32).abs() < 1e-3);
        assert!((t_end.vx - 12.0).abs() < 1e-2);
    }

    #[test]
    fn test_hermite_smoothing_snap_on_teleport() {
        let mut wv = ClientWorldView::new();
        let now = Instant::now();

        // Spawn at origin
        wv.upsert_entity_with_time(
            99,
            0,
            0,
            0,
            0,
            QuantizedCellCoord::new(0, 0, 0),
            QuantizedYaw::from_degrees(0.0),
            0,
            Vec3Fix::ZERO,
            now,
        );

        // Teleport 500 meters away (exceeding SEAM_SNAP_DISTANCE_THRESHOLD)
        let later = now + std::time::Duration::from_millis(50);
        wv.upsert_entity_with_time(
            99,
            0,
            10,
            0,
            0,
            QuantizedCellCoord::new(0, 0, 0),
            QuantizedYaw::from_degrees(0.0),
            0,
            Vec3Fix::ZERO,
            later,
        );

        let entity = wv.get_entity(99).expect("entity exists");
        let t0 = entity.extrapolate_transform(0.0);
        let (p1_x, _, _) = entity.global_position().to_f64();

        // Must snap instantly to new position without smoothing from origin
        assert!((t0.x - p1_x as f32).abs() < 1e-3);
        assert!(t0.x > 300.0);
    }
}
