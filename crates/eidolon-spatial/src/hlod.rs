//! Hierarchical Level of Detail (HLOD) and macro terrain heightfield grid.
//!
//! Provides a 3-tier spatial hierarchy:
//! - Macro-Grid (512m tiles): Terrain heightfields and sector streaming manifests.
//! - Meso-Grid (64m cells): Static player structures and compound BVHs.
//! - Micro-Grid (16m cells): Dynamic kinematic combatants and fast collision checks.
//!
//! Delivers sub-microsecond ray-terrain line of sight checks and bilinear ground clamping.

use std::collections::HashMap;

use eidolon_core::fixed::{Fixed64, Vec3Fix};

use crate::bvh::RayHit;

/// Edge length of a macro terrain tile in meters (512.0m x 512.0m).
pub const MACRO_TILE_EDGE_METERS: f64 = 512.0;

/// Number of elevation sample vertices along each axis of a macro tile (17x17 grid = 289 vertices, 32m spacing).
pub const TERRAIN_SAMPLES_PER_AXIS: usize = 17;

/// Total elevation samples in a single macro terrain tile.
pub const TOTAL_TERRAIN_SAMPLES: usize = TERRAIN_SAMPLES_PER_AXIS * TERRAIN_SAMPLES_PER_AXIS;

/// Discrete 2D coordinate identifying a 512m macro terrain tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct MacroTileCoord {
    /// Tile coordinate along X axis.
    pub tile_x: i32,
    /// Tile coordinate along Z axis.
    pub tile_z: i32,
}

impl MacroTileCoord {
    /// Computes the macro tile coordinate containing world coordinates `(x, z)`.
    pub fn from_world_coords(x: Fixed64, z: Fixed64) -> Self {
        let x_m = x.to_f64();
        let z_m = z.to_f64();
        let tx = (x_m / MACRO_TILE_EDGE_METERS).floor() as i32;
        let tz = (z_m / MACRO_TILE_EDGE_METERS).floor() as i32;
        Self {
            tile_x: tx,
            tile_z: tz,
        }
    }
}

/// A 512m x 512m macro terrain tile with 17x17 elevation samples and bounding AABB.
#[derive(Debug, Clone)]
pub struct TerrainTile {
    /// Discrete tile coordinate.
    pub coord: MacroTileCoord,
    /// Minimum world elevation within this tile.
    pub min_elevation: Fixed64,
    /// Maximum world elevation within this tile.
    pub max_elevation: Fixed64,
    /// 17x17 elevation samples in meters (row-major: Z outer, X inner).
    pub elevation_samples: [Fixed64; TOTAL_TERRAIN_SAMPLES],
}

impl TerrainTile {
    /// Creates a flat terrain tile at fixed elevation.
    pub fn new_flat(coord: MacroTileCoord, elevation: Fixed64) -> Self {
        Self {
            coord,
            min_elevation: elevation,
            max_elevation: elevation,
            elevation_samples: [elevation; TOTAL_TERRAIN_SAMPLES],
        }
    }

    /// Creates a terrain tile with specified elevation samples.
    pub fn from_samples(coord: MacroTileCoord, samples: [Fixed64; TOTAL_TERRAIN_SAMPLES]) -> Self {
        let mut min_e = samples[0];
        let mut max_e = samples[0];
        for &s in &samples[1..] {
            if s < min_e {
                min_e = s;
            }
            if s > max_e {
                max_e = s;
            }
        }
        Self {
            coord,
            min_elevation: min_e,
            max_elevation: max_e,
            elevation_samples: samples,
        }
    }

    /// World-space minimum AABB bound for broad-phase ray testing.
    pub fn min_bound(&self) -> Vec3Fix {
        Vec3Fix {
            x: Fixed64::from_f64(self.coord.tile_x as f64 * MACRO_TILE_EDGE_METERS),
            y: self.min_elevation,
            z: Fixed64::from_f64(self.coord.tile_z as f64 * MACRO_TILE_EDGE_METERS),
        }
    }

    /// World-space maximum AABB bound for broad-phase ray testing.
    pub fn max_bound(&self) -> Vec3Fix {
        Vec3Fix {
            x: Fixed64::from_f64((self.coord.tile_x + 1) as f64 * MACRO_TILE_EDGE_METERS),
            y: self.max_elevation,
            z: Fixed64::from_f64((self.coord.tile_z + 1) as f64 * MACRO_TILE_EDGE_METERS),
        }
    }

    /// Samples ground elevation at tile-local coordinates `(local_x, local_z)` using bilinear interpolation.
    /// `local_x` and `local_z` are in meters $[0.0, 512.0]$.
    pub fn sample_elevation(&self, local_x: Fixed64, local_z: Fixed64) -> Fixed64 {
        let lx = local_x.to_f64().clamp(0.0, MACRO_TILE_EDGE_METERS);
        let lz = local_z.to_f64().clamp(0.0, MACRO_TILE_EDGE_METERS);

        let cell_size = MACRO_TILE_EDGE_METERS / (TERRAIN_SAMPLES_PER_AXIS - 1) as f64; // 32.0m

        let gx = (lx / cell_size).floor() as usize;
        let gz = (lz / cell_size).floor() as usize;

        let gx0 = gx.min(TERRAIN_SAMPLES_PER_AXIS - 2);
        let gz0 = gz.min(TERRAIN_SAMPLES_PER_AXIS - 2);
        let gx1 = gx0 + 1;
        let gz1 = gz0 + 1;

        let fx = (lx - (gx0 as f64 * cell_size)) / cell_size;
        let fz = (lz - (gz0 as f64 * cell_size)) / cell_size;

        let h00 = self.elevation_samples[gz0 * TERRAIN_SAMPLES_PER_AXIS + gx0].to_f64();
        let h10 = self.elevation_samples[gz0 * TERRAIN_SAMPLES_PER_AXIS + gx1].to_f64();
        let h01 = self.elevation_samples[gz1 * TERRAIN_SAMPLES_PER_AXIS + gx0].to_f64();
        let h11 = self.elevation_samples[gz1 * TERRAIN_SAMPLES_PER_AXIS + gx1].to_f64();

        // Bilinear interpolation
        let top = h00 * (1.0 - fx) + h10 * fx;
        let bot = h01 * (1.0 - fx) + h11 * fx;
        let interp = top * (1.0 - fz) + bot * fz;

        Fixed64::from_f64(interp)
    }

    /// Raycasts against this terrain tile.
    ///
    /// Evaluates broad-phase AABB first. If hit, steps across the heightfield
    /// using uniform intervals to identify surface intersection.
    pub fn raycast(
        &self,
        ray_origin: Vec3Fix,
        ray_dir: Vec3Fix,
        max_dist: Fixed64,
    ) -> Option<RayHit> {
        let min_b = self.min_bound();
        let max_b = self.max_bound();

        // Broad-phase: slab AABB intersection
        let min_x = min_b.x.to_f64();
        let max_x = max_b.x.to_f64();
        let min_y = min_b.y.to_f64();
        let max_y = max_b.y.to_f64();
        let min_z = min_b.z.to_f64();
        let max_z = max_b.z.to_f64();

        let ox = ray_origin.x.to_f64();
        let oy = ray_origin.y.to_f64();
        let oz = ray_origin.z.to_f64();

        let dx = ray_dir.x.to_f64();
        let dy = ray_dir.y.to_f64();
        let dz = ray_dir.z.to_f64();

        let mut t_min = 0.0f64;
        let mut t_max = max_dist.to_f64();

        // X slab
        if dx.abs() > 1e-6 {
            let tx1 = (min_x - ox) / dx;
            let tx2 = (max_x - ox) / dx;
            t_min = t_min.max(tx1.min(tx2));
            t_max = t_max.min(tx1.max(tx2));
        } else if ox < min_x || ox > max_x {
            return None;
        }

        // Y slab
        if dy.abs() > 1e-6 {
            let ty1 = (min_y - oy) / dy;
            let ty2 = (max_y - oy) / dy;
            t_min = t_min.max(ty1.min(ty2));
            t_max = t_max.min(ty1.max(ty2));
        } else if oy < min_y || oy > max_y {
            return None;
        }

        // Z slab
        if dz.abs() > 1e-6 {
            let tz1 = (min_z - oz) / dz;
            let tz2 = (max_z - oz) / dz;
            t_min = t_min.max(tz1.min(tz2));
            t_max = t_max.min(tz1.max(tz2));
        } else if oz < min_z || oz > max_z {
            return None;
        }

        if t_min > t_max || t_max < 0.0 {
            return None;
        }

        // Narrow-phase ray march (step size 8.0 meters)
        let step_size = 8.0f64;
        let mut t = t_min.max(0.0);
        let tile_origin_x = self.coord.tile_x as f64 * MACRO_TILE_EDGE_METERS;
        let tile_origin_z = self.coord.tile_z as f64 * MACRO_TILE_EDGE_METERS;

        while t <= t_max {
            let rx = ox + dx * t;
            let ry = oy + dy * t;
            let rz = oz + dz * t;

            let lx = Fixed64::from_f64(rx - tile_origin_x);
            let lz = Fixed64::from_f64(rz - tile_origin_z);
            let terrain_y = self.sample_elevation(lx, lz).to_f64();

            if ry <= terrain_y {
                // Ray penetrated or touched ground surface
                let hit_point = Vec3Fix::from_f64(rx, terrain_y, rz);
                let hit_distance = Fixed64::from_f64(t);
                let normal = Vec3Fix::from_f64(0.0, 1.0, 0.0); // Upward normal default
                return Some(RayHit {
                    piece_id: 0,
                    distance: hit_distance,
                    hit_point,
                    normal,
                });
            }

            t += step_size;
        }

        None
    }
}

/// Central manager organizing macro terrain tiles across continental world maps.
#[derive(Debug, Default)]
pub struct HlodGrid {
    tiles: HashMap<MacroTileCoord, TerrainTile>,
}

impl HlodGrid {
    /// Creates a new empty HLOD grid.
    pub fn new() -> Self {
        Self {
            tiles: HashMap::new(),
        }
    }

    /// Inserts or replaces a macro terrain tile.
    pub fn insert_tile(&mut self, tile: TerrainTile) {
        self.tiles.insert(tile.coord, tile);
    }

    /// Returns the total number of resident terrain tiles.
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Samples ground elevation at world coordinates `(world_x, world_z)`.
    pub fn sample_elevation(&self, world_x: Fixed64, world_z: Fixed64) -> Option<Fixed64> {
        let coord = MacroTileCoord::from_world_coords(world_x, world_z);
        let tile = self.tiles.get(&coord)?;

        let tile_origin_x = Fixed64::from_f64(coord.tile_x as f64 * MACRO_TILE_EDGE_METERS);
        let tile_origin_z = Fixed64::from_f64(coord.tile_z as f64 * MACRO_TILE_EDGE_METERS);

        let local_x = world_x - tile_origin_x;
        let local_z = world_z - tile_origin_z;

        Some(tile.sample_elevation(local_x, local_z))
    }

    /// Executes a raycast against all resident terrain tiles, returning the closest intersection.
    pub fn raycast(
        &self,
        ray_origin: Vec3Fix,
        ray_dir: Vec3Fix,
        max_distance: Fixed64,
    ) -> Option<RayHit> {
        let mut closest_hit: Option<RayHit> = None;

        for tile in self.tiles.values() {
            if let Some(hit) = tile.raycast(ray_origin, ray_dir, max_distance) {
                let is_closer = match closest_hit {
                    Some(ref current) => hit.distance < current.distance,
                    None => true,
                };
                if is_closer {
                    closest_hit = Some(hit);
                }
            }
        }

        closest_hit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terrain_tile_flat_ground_clamping() {
        let coord = MacroTileCoord {
            tile_x: 0,
            tile_z: 0,
        };
        let tile = TerrainTile::new_flat(coord, Fixed64::from_f64(25.0));

        let h1 = tile.sample_elevation(Fixed64::from_f64(100.0), Fixed64::from_f64(100.0));
        assert_eq!(h1, Fixed64::from_f64(25.0));

        let h2 = tile.sample_elevation(Fixed64::from_f64(256.0), Fixed64::from_f64(500.0));
        assert_eq!(h2, Fixed64::from_f64(25.0));
    }

    #[test]
    fn test_terrain_tile_bilinear_interpolation() {
        let coord = MacroTileCoord {
            tile_x: 0,
            tile_z: 0,
        };
        let mut samples = [Fixed64::ZERO; TOTAL_TERRAIN_SAMPLES];

        // Set corner samples of first cell (0,0 to 32,32)
        // (0,0) = 0m, (32,0) = 10m, (0,32) = 0m, (32,32) = 10m
        samples[0] = Fixed64::ZERO;
        samples[1] = Fixed64::from_f64(10.0);
        samples[TERRAIN_SAMPLES_PER_AXIS] = Fixed64::ZERO;
        samples[TERRAIN_SAMPLES_PER_AXIS + 1] = Fixed64::from_f64(10.0);

        let tile = TerrainTile::from_samples(coord, samples);

        // At midpoint lx = 16.0m, lz = 16.0m: elevation should be exactly 5.0m
        let mid = tile.sample_elevation(Fixed64::from_f64(16.0), Fixed64::from_f64(16.0));
        assert_eq!(mid, Fixed64::from_f64(5.0));
    }

    #[test]
    fn test_terrain_raycast_hit_and_miss() {
        let coord = MacroTileCoord {
            tile_x: 0,
            tile_z: 0,
        };
        let tile = TerrainTile::new_flat(coord, Fixed64::from_f64(10.0));

        // Raycast straight down from (100, 50, 100) facing -Y
        let ray_origin = Vec3Fix::from_f64(100.0, 50.0, 100.0);
        let ray_dir = Vec3Fix::from_f64(0.0, -1.0, 0.0);
        let max_dist = Fixed64::from_f64(100.0);

        let hit = tile
            .raycast(ray_origin, ray_dir, max_dist)
            .expect("Ray should hit flat ground");
        // Ground is at y=10.0, origin at y=50.0 => distance = 40.0m
        assert_eq!(hit.hit_point.y, Fixed64::from_f64(10.0));
        assert_eq!(hit.distance, Fixed64::from_f64(40.0));

        // Raycast pointing upward (+Y) misses terrain
        let ray_up_dir = Vec3Fix::from_f64(0.0, 1.0, 0.0);
        let miss = tile.raycast(ray_origin, ray_up_dir, max_dist);
        assert!(miss.is_none());
    }
}
