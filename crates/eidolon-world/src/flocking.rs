//! Boids-style spatial flocking and separation routines for NPC swarms.
//!
//! Prevents pack mobs from collapsing into a single physical point during combat
//! by generating smooth repulsive steering vectors with zero heap allocations.

/// Computes a 2D repulsive separation vector away from nearby flocking neighbors.
///
/// If no neighbors are within `separation_radius`, returns `(0.0, 0.0)`.
#[inline]
pub fn compute_separation(
    my_pos: (f32, f32),
    neighbors: &[(f32, f32)],
    separation_radius: f32,
) -> (f32, f32) {
    if neighbors.is_empty() || separation_radius <= 0.001 {
        return (0.0, 0.0);
    }

    let mut repel_x = 0.0f32;
    let mut repel_z = 0.0f32;
    let radius_sq = separation_radius * separation_radius;

    for &neighbor_pos in neighbors {
        let dx = my_pos.0 - neighbor_pos.0;
        let dz = my_pos.1 - neighbor_pos.1;
        let dist_sq = dx * dx + dz * dz;

        // Apply repulsion only if within separation radius and not self (dist_sq > 0)
        if dist_sq < radius_sq && dist_sq > 0.0001 {
            let dist = dist_sq.sqrt();
            let strength = (separation_radius - dist) / separation_radius;
            // Unit vector pointing away from neighbor, scaled by proximity strength
            repel_x += (dx / dist) * strength;
            repel_z += (dz / dist) * strength;
        }
    }

    // Normalize final steering vector if magnitude > 1.0
    let mag_sq = repel_x * repel_x + repel_z * repel_z;
    if mag_sq > 1.0 {
        let mag = mag_sq.sqrt();
        repel_x /= mag;
        repel_z /= mag;
    }

    (repel_x, repel_z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_repulsion_when_neighbors_outside_radius() {
        let my_pos = (0.0, 0.0);
        let neighbors = [(10.0, 0.0), (-10.0, 0.0)];
        let sep = compute_separation(my_pos, &neighbors, 2.0); // 2m radius
        assert_eq!(sep, (0.0, 0.0));
    }

    #[test]
    fn test_repulsion_pushes_directly_away() {
        let my_pos = (0.0, 0.0);
        let neighbors = [(1.0, 0.0)]; // Neighbor 1m to the right (+X)
        let sep = compute_separation(my_pos, &neighbors, 2.0);

        // Repulsion should push directly to the left (-X)
        assert!(sep.0 < -0.4);
        assert_eq!(sep.1, 0.0);
    }

    #[test]
    fn test_balanced_neighbors_cancel_out() {
        let my_pos = (0.0, 0.0);
        let neighbors = [(1.0, 0.0), (-1.0, 0.0)]; // Symmetric neighbors on +X and -X
        let sep = compute_separation(my_pos, &neighbors, 2.0);

        assert!(sep.0.abs() < 0.001);
        assert!(sep.1.abs() < 0.001);
    }
}
