//! Server-side compound static BVH welding and hierarchical raycasting.
//!
//! Welds hundreds or thousands of connected freeform building pieces sharing a foundation
//! into a single compound structure with an outer Axis-Aligned Bounding Box (AABB)
//! and flat leaf nodes for sub-microsecond raycasts and projectile collision queries.

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::structure::PieceType;

/// Result of a successful raycast intersection against a building structure piece.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// Unique identifier of the struck building piece.
    pub piece_id: u32,
    /// Distance from ray origin to impact point.
    pub distance: Fixed64,
    /// Exact continuous 32.32 fixed-point coordinate of impact.
    pub hit_point: Vec3Fix,
    /// Outward surface normal vector at impact point.
    pub normal: Vec3Fix,
}

/// Bounding volume representation of a single leaf building piece.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BvhLeafPiece {
    /// Unique piece identifier within the compound structure.
    pub piece_id: u32,
    /// Structural classification of the piece.
    pub piece_type: PieceType,
    /// Center position in continuous world space.
    pub center: Vec3Fix,
    /// Minimum corner of the piece AABB.
    pub min_bounds: Vec3Fix,
    /// Maximum corner of the piece AABB.
    pub max_bounds: Vec3Fix,
}

/// Welded compound structure containing multiple building pieces.
///
/// Maintains a single unified outer AABB enclosing all child pieces for $O(1)$ broad-phase
/// rejection during raycasts, projectile trajectories, and line-of-sight checks.
#[derive(Debug, Clone)]
pub struct CompoundStructure {
    /// Authoritative structure identifier (e.g. building or compound ID).
    pub structure_id: u32,
    /// Minimum corner of the outer compound AABB.
    pub min_bounds: Vec3Fix,
    /// Maximum corner of the outer compound AABB.
    pub max_bounds: Vec3Fix,
    /// Contiguous leaf pieces composing the structure.
    pub pieces: Vec<BvhLeafPiece>,
}

impl CompoundStructure {
    /// Creates a new empty compound structure.
    pub fn new(structure_id: u32) -> Self {
        Self {
            structure_id,
            min_bounds: Vec3Fix::from_f64(f64::MAX, f64::MAX, f64::MAX),
            max_bounds: Vec3Fix::from_f64(f64::MIN, f64::MIN, f64::MIN),
            pieces: Vec::new(),
        }
    }

    /// Returns the total number of welded pieces in this compound structure.
    pub fn piece_count(&self) -> usize {
        self.pieces.len()
    }

    /// Adds a building piece to the compound structure, expanding the outer AABB in $O(1)$.
    pub fn add_piece(&mut self, piece_id: u32, piece_type: PieceType, center: Vec3Fix) {
        let half = piece_type.half_extents();
        let min_b = center - half;
        let max_b = center + half;

        // Expand outer compound AABB
        if self.pieces.is_empty() {
            self.min_bounds = min_b;
            self.max_bounds = max_b;
        } else {
            self.min_bounds.x = self.min_bounds.x.min(min_b.x);
            self.min_bounds.y = self.min_bounds.y.min(min_b.y);
            self.min_bounds.z = self.min_bounds.z.min(min_b.z);

            self.max_bounds.x = self.max_bounds.x.max(max_b.x);
            self.max_bounds.y = self.max_bounds.y.max(max_b.y);
            self.max_bounds.z = self.max_bounds.z.max(max_b.z);
        }

        self.pieces.push(BvhLeafPiece {
            piece_id,
            piece_type,
            center,
            min_bounds: min_b,
            max_bounds: max_b,
        });
    }

    /// Removes a piece by ID and recalculates the outer compound AABB if necessary.
    pub fn remove_piece(&mut self, piece_id: u32) -> bool {
        if let Some(pos) = self.pieces.iter().position(|p| p.piece_id == piece_id) {
            self.pieces.swap_remove(pos);
            self.recompute_bounds();
            true
        } else {
            false
        }
    }

    /// Recomputes outer compound AABB from all remaining leaf pieces.
    pub fn recompute_bounds(&mut self) {
        if self.pieces.is_empty() {
            self.min_bounds = Vec3Fix::from_f64(f64::MAX, f64::MAX, f64::MAX);
            self.max_bounds = Vec3Fix::from_f64(f64::MIN, f64::MIN, f64::MIN);
            return;
        }

        self.min_bounds = self.pieces[0].min_bounds;
        self.max_bounds = self.pieces[0].max_bounds;

        for p in &self.pieces[1..] {
            self.min_bounds.x = self.min_bounds.x.min(p.min_bounds.x);
            self.min_bounds.y = self.min_bounds.y.min(p.min_bounds.y);
            self.min_bounds.z = self.min_bounds.z.min(p.min_bounds.z);

            self.max_bounds.x = self.max_bounds.x.max(p.max_bounds.x);
            self.max_bounds.y = self.max_bounds.y.max(p.max_bounds.y);
            self.max_bounds.z = self.max_bounds.z.max(p.max_bounds.z);
        }
    }

    /// Executes a hierarchical raycast against the compound structure.
    ///
    /// Evaluates broad-phase outer AABB rejection first. If hit, performs narrow-phase
    /// leaf testing, returning the closest intersection within `max_distance`.
    pub fn raycast(
        &self,
        ray_origin: Vec3Fix,
        ray_dir: Vec3Fix,
        max_distance: Fixed64,
    ) -> Option<RayHit> {
        if self.pieces.is_empty() {
            return None;
        }

        // Broad-phase: test against outer compound AABB
        if !intersect_aabb(
            ray_origin,
            ray_dir,
            self.min_bounds,
            self.max_bounds,
            max_distance,
        ) {
            return None;
        }

        // Narrow-phase: find closest leaf intersection
        let mut closest_hit: Option<RayHit> = None;
        let mut closest_dist = max_distance;

        for piece in &self.pieces {
            if let Some(hit) = raycast_leaf_aabb(ray_origin, ray_dir, piece, closest_dist) {
                if hit.distance < closest_dist {
                    closest_dist = hit.distance;
                    closest_hit = Some(hit);
                }
            }
        }

        closest_hit
    }
}

/// Tests whether a ray intersects an Axis-Aligned Bounding Box within max_dist using the slab method.
fn intersect_aabb(
    origin: Vec3Fix,
    dir: Vec3Fix,
    min_b: Vec3Fix,
    max_b: Vec3Fix,
    max_dist: Fixed64,
) -> bool {
    let mut tmin = Fixed64::ZERO;
    let mut tmax = max_dist;

    // X axis slab
    if dir.x.abs() > Fixed64::from_f64(0.00001) {
        let inv_d = Fixed64::ONE / dir.x;
        let mut t1 = (min_b.x - origin.x) * inv_d;
        let mut t2 = (max_b.x - origin.x) * inv_d;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        tmin = tmin.max(t1);
        tmax = tmax.min(t2);
        if tmin > tmax {
            return false;
        }
    } else if origin.x < min_b.x || origin.x > max_b.x {
        return false;
    }

    // Y axis slab
    if dir.y.abs() > Fixed64::from_f64(0.00001) {
        let inv_d = Fixed64::ONE / dir.y;
        let mut t1 = (min_b.y - origin.y) * inv_d;
        let mut t2 = (max_b.y - origin.y) * inv_d;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        tmin = tmin.max(t1);
        tmax = tmax.min(t2);
        if tmin > tmax {
            return false;
        }
    } else if origin.y < min_b.y || origin.y > max_b.y {
        return false;
    }

    // Z axis slab
    if dir.z.abs() > Fixed64::from_f64(0.00001) {
        let inv_d = Fixed64::ONE / dir.z;
        let mut t1 = (min_b.z - origin.z) * inv_d;
        let mut t2 = (max_b.z - origin.z) * inv_d;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        tmin = tmin.max(t1);
        tmax = tmax.min(t2);
        if tmin > tmax {
            return false;
        }
    } else if origin.z < min_b.z || origin.z > max_b.z {
        return false;
    }

    true
}

/// Raycasts a single leaf piece AABB and returns the closest hit point and surface normal.
fn raycast_leaf_aabb(
    origin: Vec3Fix,
    dir: Vec3Fix,
    piece: &BvhLeafPiece,
    max_dist: Fixed64,
) -> Option<RayHit> {
    let mut tmin = Fixed64::ZERO;
    let mut tmax = max_dist;
    let mut hit_normal = Vec3Fix::ZERO;

    // X slab
    if dir.x.abs() > Fixed64::from_f64(0.00001) {
        let inv_d = Fixed64::ONE / dir.x;
        let mut t1 = (piece.min_bounds.x - origin.x) * inv_d;
        let mut t2 = (piece.max_bounds.x - origin.x) * inv_d;
        let mut norm = Vec3Fix::from_f64(-1.0, 0.0, 0.0);

        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
            norm = Vec3Fix::from_f64(1.0, 0.0, 0.0);
        }

        if t1 > tmin {
            tmin = t1;
            hit_normal = norm;
        }
        tmax = tmax.min(t2);
        if tmin > tmax {
            return None;
        }
    } else if origin.x < piece.min_bounds.x || origin.x > piece.max_bounds.x {
        return None;
    }

    // Y slab
    if dir.y.abs() > Fixed64::from_f64(0.00001) {
        let inv_d = Fixed64::ONE / dir.y;
        let mut t1 = (piece.min_bounds.y - origin.y) * inv_d;
        let mut t2 = (piece.max_bounds.y - origin.y) * inv_d;
        let mut norm = Vec3Fix::from_f64(0.0, -1.0, 0.0);

        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
            norm = Vec3Fix::from_f64(0.0, 1.0, 0.0);
        }

        if t1 > tmin {
            tmin = t1;
            hit_normal = norm;
        }
        tmax = tmax.min(t2);
        if tmin > tmax {
            return None;
        }
    } else if origin.y < piece.min_bounds.y || origin.y > piece.max_bounds.y {
        return None;
    }

    // Z slab
    if dir.z.abs() > Fixed64::from_f64(0.00001) {
        let inv_d = Fixed64::ONE / dir.z;
        let mut t1 = (piece.min_bounds.z - origin.z) * inv_d;
        let mut t2 = (piece.max_bounds.z - origin.z) * inv_d;
        let mut norm = Vec3Fix::from_f64(0.0, 0.0, -1.0);

        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
            norm = Vec3Fix::from_f64(0.0, 0.0, 1.0);
        }

        if t1 > tmin {
            tmin = t1;
            hit_normal = norm;
        }
        tmax = tmax.min(t2);
        if tmin > tmax {
            return None;
        }
    } else if origin.z < piece.min_bounds.z || origin.z > piece.max_bounds.z {
        return None;
    }

    if tmin <= max_dist && tmin >= Fixed64::ZERO {
        let hit_point = origin + dir * tmin;
        Some(RayHit {
            piece_id: piece.piece_id,
            distance: tmin,
            hit_point,
            normal: hit_normal,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compound_structure_bounds_expansion() {
        let mut compound = CompoundStructure::new(101);
        assert_eq!(compound.piece_count(), 0);

        // Add foundation at (0, 0, 0), half-extents (2.0, 0.5, 2.0)
        compound.add_piece(1, PieceType::Foundation, Vec3Fix::ZERO);
        assert_eq!(compound.piece_count(), 1);
        assert_eq!(compound.min_bounds, Vec3Fix::from_f64(-2.0, -0.5, -2.0));
        assert_eq!(compound.max_bounds, Vec3Fix::from_f64(2.0, 0.5, 2.0));

        // Add wall at (0, 2.0, 2.0), half-extents (2.0, 1.5, 0.1)
        compound.add_piece(2, PieceType::Wall, Vec3Fix::from_f64(0.0, 2.0, 2.0));
        assert_eq!(compound.piece_count(), 2);
        assert_eq!(compound.min_bounds, Vec3Fix::from_f64(-2.0, -0.5, -2.0));
        assert_eq!(compound.max_bounds, Vec3Fix::from_f64(2.0, 3.5, 2.1));
    }

    #[test]
    fn test_compound_structure_raycast_hit_and_miss() {
        let mut compound = CompoundStructure::new(102);
        compound.add_piece(1, PieceType::Wall, Vec3Fix::from_f64(0.0, 1.5, 10.0));

        // Ray aimed directly at wall from origin along +Z
        let ray_orig = Vec3Fix::from_f64(0.0, 1.5, 0.0);
        let ray_dir = Vec3Fix::from_f64(0.0, 0.0, 1.0);
        let hit = compound.raycast(ray_orig, ray_dir, Fixed64::from_i32(50));

        assert!(hit.is_some());
        let h = hit.unwrap();
        assert_eq!(h.piece_id, 1);
        // Wall is at z = 10, half-thickness 0.1 => front face at z = 9.9
        assert!((h.distance.to_f64() - 9.9).abs() < 0.01);

        // Ray aimed away from wall (broad-phase rejection)
        let miss_dir = Vec3Fix::from_f64(1.0, 0.0, 0.0);
        let miss = compound.raycast(ray_orig, miss_dir, Fixed64::from_i32(50));
        assert!(miss.is_none());
    }

    #[test]
    fn test_compound_structure_piece_removal() {
        let mut compound = CompoundStructure::new(103);
        compound.add_piece(1, PieceType::Foundation, Vec3Fix::ZERO);
        compound.add_piece(2, PieceType::Wall, Vec3Fix::from_f64(10.0, 1.5, 0.0));
        assert_eq!(compound.piece_count(), 2);

        assert!(compound.remove_piece(2));
        assert_eq!(compound.piece_count(), 1);
        // Bounds should shrink back to only foundation (x max = 2.0 instead of 12.0)
        assert_eq!(compound.max_bounds.x, Fixed64::from_i32(2));
    }
}
