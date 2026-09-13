//! Player structure management, modular prefabs, freeform construction, and stability cascades.
//!
//! Provides decoupled dual-tier spatial storage for static buildings, Valheim-style
//! load-bearing stability recalculation, cascading collapse upon foundation destruction,
//! and background maintenance decay loops with zero overhead on the 20 Hz simulation tick.

use std::collections::{HashMap, VecDeque};

use eidolon_core::fixed::Vec3Fix;
use eidolon_core::quant::QuantizedYaw;
use eidolon_core::structure::{
    calculate_stability, MaterialType, PieceType, SnapSocket, StructurePieceData, MAX_STABILITY,
};
use eidolon_spatial::bvh::CompoundStructure;

use crate::error::WorldError;

/// An individual building piece within a player structure.
#[derive(Debug, Clone, PartialEq)]
pub struct StructurePiece {
    /// Unique piece identifier within the world.
    pub piece_id: u32,
    /// Parent piece identifier this piece is snapped to (0 if root foundation).
    pub parent_piece_id: u32,
    /// Compact 3-byte bitpacked piece metadata.
    pub piece_data: StructurePieceData,
    /// World position of the piece center.
    pub world_position: Vec3Fix,
    /// Facing heading discrete angle.
    pub world_yaw: QuantizedYaw,
    /// Structural load-bearing stability score (0..100).
    pub stability: u8,
    /// Current durability hit points.
    pub health: u32,
    /// Maximum durability hit points.
    pub max_health: u32,
}

/// An active player structure (modular prefab or welded freeform compound).
#[derive(Debug, Clone)]
pub struct StructureInstance {
    /// Unique structure identifier.
    pub id: u32,
    /// Account identifier of the builder / owner.
    pub owner_account_id: u64,
    /// True if prefabricated modular building (SWG style), false if freeform.
    pub is_prefab: bool,
    /// Prefabricated catalog template ID (e.g. SmallHouse, GuildHall, Harvester).
    pub prefab_type_id: u32,
    /// Root world position.
    pub world_position: Vec3Fix,
    /// Facing heading discrete angle.
    pub world_yaw: QuantizedYaw,
    /// Child building pieces composing this structure.
    pub pieces: HashMap<u32, StructurePiece>,
    /// Server-side welded Compound BVH for sub-microsecond raycasts.
    pub compound: CompoundStructure,
    /// Associated pocket-dimension interior cell ID (if building has an interior).
    pub interior_cell_id: Option<u32>,
    /// Current overall structural health points.
    pub health: u32,
    /// Maximum overall structural health points.
    pub max_health: u32,
    /// Last simulation tick when maintenance decay was evaluated.
    pub last_maintenance_tick: u64,
}

/// Central manager coordinating all static player structures across the world.
///
/// Decoupled from the 20 Hz kinematic simulation tick loop: structures reside in
/// dedicated spatial partitions and do not consume CPU cycles during player movement.
#[derive(Debug, Default)]
pub struct StructureManager {
    structures: HashMap<u32, StructureInstance>,
    next_structure_id: u32,
    next_piece_id: u32,
}

impl StructureManager {
    /// Creates a new empty structure manager.
    pub fn new() -> Self {
        Self {
            structures: HashMap::new(),
            next_structure_id: 1,
            next_piece_id: 1,
        }
    }

    /// Returns the total number of active player structures in the world.
    pub fn total_structure_count(&self) -> usize {
        self.structures.len()
    }

    /// Returns the total number of building pieces across all active structures.
    pub fn total_piece_count(&self) -> usize {
        self.structures.values().map(|s| s.pieces.len()).sum()
    }

    /// Retrieves an immutable reference to a structure by ID.
    pub fn get_structure(&self, structure_id: u32) -> Option<&StructureInstance> {
        self.structures.get(&structure_id)
    }

    /// Retrieves a mutable reference to a structure by ID.
    pub fn get_structure_mut(&mut self, structure_id: u32) -> Option<&mut StructureInstance> {
        self.structures.get_mut(&structure_id)
    }

    /// Places a prefabricated modular building (SWG house, harvester, vendor).
    pub fn create_prefab(
        &mut self,
        owner_account_id: u64,
        prefab_type_id: u32,
        position: Vec3Fix,
        yaw: QuantizedYaw,
        interior_cell_id: Option<u32>,
    ) -> Result<u32, WorldError> {
        let structure_id = self.next_structure_id;
        self.next_structure_id = self.next_structure_id.wrapping_add(1);

        let piece_id = self.next_piece_id;
        self.next_piece_id = self.next_piece_id.wrapping_add(1);

        let mut compound = CompoundStructure::new(structure_id);
        compound.add_piece(piece_id, PieceType::Foundation, position);

        let piece_data = StructurePieceData::new(
            PieceType::Foundation,
            MaterialType::Stone,
            SnapSocket::Center,
            0,
        );

        let root_piece = StructurePiece {
            piece_id,
            parent_piece_id: 0,
            piece_data,
            world_position: position,
            world_yaw: yaw,
            stability: MAX_STABILITY,
            health: 5000,
            max_health: 5000,
        };

        let mut pieces = HashMap::new();
        pieces.insert(piece_id, root_piece);

        let instance = StructureInstance {
            id: structure_id,
            owner_account_id,
            is_prefab: true,
            prefab_type_id,
            world_position: position,
            world_yaw: yaw,
            pieces,
            compound,
            interior_cell_id,
            health: 5000,
            max_health: 5000,
            last_maintenance_tick: 0,
        };

        self.structures.insert(structure_id, instance);
        Ok(structure_id)
    }

    /// Places a new grounded foundation establishing a freeform construction group.
    pub fn place_foundation(
        &mut self,
        owner_account_id: u64,
        position: Vec3Fix,
        yaw: QuantizedYaw,
        material: MaterialType,
    ) -> Result<(u32, u32), WorldError> {
        let structure_id = self.next_structure_id;
        self.next_structure_id = self.next_structure_id.wrapping_add(1);

        let piece_id = self.next_piece_id;
        self.next_piece_id = self.next_piece_id.wrapping_add(1);

        let mut compound = CompoundStructure::new(structure_id);
        compound.add_piece(piece_id, PieceType::Foundation, position);

        let piece_data =
            StructurePieceData::new(PieceType::Foundation, material, SnapSocket::Center, 0);

        let root_piece = StructurePiece {
            piece_id,
            parent_piece_id: 0,
            piece_data,
            world_position: position,
            world_yaw: yaw,
            stability: MAX_STABILITY,
            health: material.max_health(),
            max_health: material.max_health(),
        };

        let mut pieces = HashMap::new();
        pieces.insert(piece_id, root_piece);

        let instance = StructureInstance {
            id: structure_id,
            owner_account_id,
            is_prefab: false,
            prefab_type_id: 0,
            world_position: position,
            world_yaw: yaw,
            pieces,
            compound,
            interior_cell_id: None,
            health: material.max_health(),
            max_health: material.max_health(),
            last_maintenance_tick: 0,
        };

        self.structures.insert(structure_id, instance);
        Ok((structure_id, piece_id))
    }

    /// Snaps and places a child building piece onto an existing parent piece.
    ///
    /// Evaluates structural load-bearing stability. If stability drops to 0, rejects
    /// placement with `WorldError::StructurallyUnsound`.
    pub fn place_piece(
        &mut self,
        structure_id: u32,
        parent_piece_id: u32,
        piece_type: PieceType,
        material: MaterialType,
        socket: SnapSocket,
        variant_flags: u8,
    ) -> Result<u32, WorldError> {
        let structure = self
            .structures
            .get_mut(&structure_id)
            .ok_or(WorldError::StructureNotFound(structure_id))?;

        let parent = structure
            .pieces
            .get(&parent_piece_id)
            .ok_or(WorldError::PieceNotFound(parent_piece_id))?;

        // Calculate stability from parent
        let child_stability =
            calculate_stability(piece_type.is_foundation(), parent.stability, material);
        if child_stability == 0 {
            return Err(WorldError::StructurallyUnsound);
        }

        // Calculate socket offset in world space
        let child_pos = parent.world_position + socket.local_offset();

        let piece_id = self.next_piece_id;
        self.next_piece_id = self.next_piece_id.wrapping_add(1);

        let piece_data = StructurePieceData::new(piece_type, material, socket, variant_flags);

        let child_piece = StructurePiece {
            piece_id,
            parent_piece_id,
            piece_data,
            world_position: child_pos,
            world_yaw: parent.world_yaw,
            stability: child_stability,
            health: material.max_health(),
            max_health: material.max_health(),
        };

        structure
            .compound
            .add_piece(piece_id, piece_type, child_pos);
        structure.pieces.insert(piece_id, child_piece);

        Ok(piece_id)
    }

    /// Destroys a building piece and triggers cascading collapse of ungrounded pieces.
    ///
    /// When a supporting wall or foundation is removed, traverses the downstream connection
    /// graph and collapses any child pieces whose stability drops to 0.
    /// Returns the list of all collapsed piece IDs.
    pub fn destroy_piece(
        &mut self,
        structure_id: u32,
        target_piece_id: u32,
    ) -> Result<Vec<u32>, WorldError> {
        let structure = self
            .structures
            .get_mut(&structure_id)
            .ok_or(WorldError::StructureNotFound(structure_id))?;

        if !structure.pieces.contains_key(&target_piece_id) {
            return Err(WorldError::PieceNotFound(target_piece_id));
        }

        // Remove initial target piece
        structure.pieces.remove(&target_piece_id);
        structure.compound.remove_piece(target_piece_id);

        let mut collapsed_ids = vec![target_piece_id];

        // Recalculate stability propagation using BFS from grounded foundations
        // 1. Find all pieces with parent == 0 (grounded roots)
        let mut stability_map: HashMap<u32, u8> = HashMap::new();
        let mut queue = VecDeque::new();

        for (pid, piece) in &structure.pieces {
            if piece.parent_piece_id == 0
                && piece.piece_data.piece_type() == Some(PieceType::Foundation)
            {
                stability_map.insert(*pid, MAX_STABILITY);
                queue.push_back(*pid);
            }
        }

        // 2. Propagate stability downward to children
        while let Some(parent_id) = queue.pop_front() {
            let parent_stab = *stability_map.get(&parent_id).unwrap_or(&0);

            for (child_id, child_piece) in &structure.pieces {
                if child_piece.parent_piece_id == parent_id {
                    let mat = child_piece
                        .piece_data
                        .material()
                        .unwrap_or(MaterialType::Wood);
                    let child_stab = calculate_stability(false, parent_stab, mat);

                    if child_stab > 0 {
                        // Check if newly discovered or higher stability path
                        let existing = *stability_map.get(child_id).unwrap_or(&0);
                        if child_stab > existing {
                            stability_map.insert(*child_id, child_stab);
                            queue.push_back(*child_id);
                        }
                    }
                }
            }
        }

        // 3. Any piece in structure.pieces without valid stability (> 0) has collapsed!
        let to_collapse: Vec<u32> = structure
            .pieces
            .keys()
            .copied()
            .filter(|pid| !stability_map.contains_key(pid))
            .collect();

        for cid in to_collapse {
            structure.pieces.remove(&cid);
            structure.compound.remove_piece(cid);
            collapsed_ids.push(cid);
        }

        Ok(collapsed_ids)
    }

    /// Completely demolishes a player structure and frees all associated memory.
    pub fn destroy_structure(&mut self, structure_id: u32) -> Result<(), WorldError> {
        if self.structures.remove(&structure_id).is_some() {
            Ok(())
        } else {
            Err(WorldError::StructureNotFound(structure_id))
        }
    }

    /// Executes decoupled background maintenance decay across all player structures.
    ///
    /// Evaluates structure decay and deletes abandoned structures whose health reaches 0.
    /// Returns the number of structures that decayed to zero and were reclaimed.
    pub fn run_maintenance_cycle(&mut self, current_tick: u64, decay_amount: u32) -> usize {
        let mut destroyed_count = 0;
        let mut to_remove = Vec::new();

        for (id, structure) in &mut self.structures {
            structure.last_maintenance_tick = current_tick;
            structure.health = structure.health.saturating_sub(decay_amount);
            if structure.health == 0 {
                to_remove.push(*id);
            }
        }

        for id in to_remove {
            self.structures.remove(&id);
            destroyed_count += 1;
        }

        destroyed_count
    }

    /// Performs a raycast against all active player structures in the world.
    ///
    /// Evaluates hierarchical compound BVHs: broad-phase compound AABB rejection
    /// followed by narrow-phase piece AABB intersection.
    pub fn raycast(
        &self,
        origin: Vec3Fix,
        dir: Vec3Fix,
        max_distance: eidolon_core::fixed::Fixed64,
    ) -> Option<eidolon_spatial::bvh::RayHit> {
        let mut closest_hit: Option<eidolon_spatial::bvh::RayHit> = None;
        for structure in self.structures.values() {
            if let Some(hit) = structure.compound.raycast(origin, dir, max_distance) {
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

    /// Returns an iterator over all active structure instances.
    pub fn iter(&self) -> impl Iterator<Item = &StructureInstance> {
        self.structures.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modular_prefab_creation() {
        let mut mgr = StructureManager::new();
        let pos = Vec3Fix::from_f64(100.0, 0.0, 100.0);
        let yaw = QuantizedYaw::from_degrees(0.0);

        let sid = mgr
            .create_prefab(1001, 42, pos, yaw, Some(9001))
            .expect("Create prefab");

        let s = mgr.get_structure(sid).expect("Get structure");
        assert_eq!(s.owner_account_id, 1001);
        assert!(s.is_prefab);
        assert_eq!(s.prefab_type_id, 42);
        assert_eq!(s.interior_cell_id, Some(9001));
        assert_eq!(s.pieces.len(), 1);
        assert_eq!(s.compound.piece_count(), 1);
    }

    #[test]
    fn test_freeform_building_and_stability_cascade() {
        let mut mgr = StructureManager::new();
        let pos = Vec3Fix::ZERO;
        let yaw = QuantizedYaw::from_degrees(0.0);

        // 1. Place stone foundation (piece 1, stability 100)
        let (sid, foundation_id) = mgr
            .place_foundation(2001, pos, yaw, MaterialType::Stone)
            .expect("Place foundation");

        // 2. Place vertical wall on foundation (piece 2, socket Top, stability 100 - 8 = 92)
        let wall1 = mgr
            .place_piece(
                sid,
                foundation_id,
                PieceType::Wall,
                MaterialType::Stone,
                SnapSocket::Top,
                0,
            )
            .expect("Place wall 1");

        // 3. Place second vertical wall on wall 1 (piece 3, socket Top, stability 92 - 8 = 84)
        let wall2 = mgr
            .place_piece(
                sid,
                wall1,
                PieceType::Wall,
                MaterialType::Stone,
                SnapSocket::Top,
                0,
            )
            .expect("Place wall 2");

        let s = mgr.get_structure(sid).expect("Get structure");
        assert_eq!(s.pieces.len(), 3);
        assert_eq!(s.compound.piece_count(), 3);

        // 4. Destroy foundation: wall 1 and wall 2 have no ground path, so both collapse!
        let collapsed = mgr
            .destroy_piece(sid, foundation_id)
            .expect("Destroy foundation");

        // Should report foundation_id, wall1, and wall2 as collapsed
        assert_eq!(collapsed.len(), 3);
        assert!(collapsed.contains(&foundation_id));
        assert!(collapsed.contains(&wall1));
        assert!(collapsed.contains(&wall2));

        let s_after = mgr.get_structure(sid).expect("Get structure");
        assert_eq!(s_after.pieces.len(), 0);
        assert_eq!(s_after.compound.piece_count(), 0);
    }

    #[test]
    fn test_structurally_unsound_piece_rejected() {
        let mut mgr = StructureManager::new();
        let pos = Vec3Fix::ZERO;
        let yaw = QuantizedYaw::from_degrees(0.0);

        // Wood foundation
        let (sid, mut parent_id) = mgr
            .place_foundation(3001, pos, yaw, MaterialType::Wood)
            .expect("Place wood foundation");

        // Wood stability decays by 15 per piece (100 -> 85 -> 70 -> 55 -> 40 -> 25 -> 10 -> 0)
        // 6 walls tall can stand, 7th wall drops to 0 and must be rejected!
        for _ in 0..6 {
            parent_id = mgr
                .place_piece(
                    sid,
                    parent_id,
                    PieceType::Wall,
                    MaterialType::Wood,
                    SnapSocket::Top,
                    0,
                )
                .expect("Place wood wall");
        }

        // 7th wall has 10 - 15 = 0 stability: must fail with StructurallyUnsound
        let err = mgr.place_piece(
            sid,
            parent_id,
            PieceType::Wall,
            MaterialType::Wood,
            SnapSocket::Top,
            0,
        );

        assert_eq!(err, Err(WorldError::StructurallyUnsound));
    }
}
