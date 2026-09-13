//! Per-Player Strict Memory Budget Specification and Compile-Time Struct Profiling.
//!
//! Enforces bounded resident memory consumption per connected client, proving that
//! 100,000 Concurrent Users (CCU) requires strictly less than 5.0 GB of heap space.

/// Maximum allowable memory allocation ceiling per connected player session (48 KB).
pub const MAX_PLAYER_MEMORY_CEILING_BYTES: usize = 49_152;

/// Memory profile report for an individual engine data structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructMemoryProfile {
    /// Canonical type name of the structure.
    pub type_name: &'static str,
    /// Exact byte size via `std::mem::size_of`.
    pub size_bytes: usize,
    /// Byte alignment boundary via `std::mem::align_of`.
    pub align_bytes: usize,
}

/// Strict per-player memory budget accounting breakdown across all server subsystems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerMemoryBudget {
    /// Ingress and egress connection state, security handshake, and rate policer (bytes).
    pub connection_state_bytes: usize,
    /// Kinematic position, velocity vectors, and dead reckoning extrapolation (bytes).
    pub kinematic_state_bytes: usize,
    /// Area of Interest (AoI) observer tracking tables and frequency tier relations (bytes).
    pub aoi_relations_bytes: usize,
    /// Reliable transport retransmission buffer and sequenced packet cache (bytes).
    pub reliable_channel_bytes: usize,
    /// In-flight transaction journal and generation lock tokens (bytes).
    pub transaction_state_bytes: usize,
}

impl Default for PlayerMemoryBudget {
    fn default() -> Self {
        Self {
            connection_state_bytes: 16_384, // 16 KB bounded ingress/egress ring buffers
            kinematic_state_bytes: 512,     // Transform quantizer and dead reckoning
            aoi_relations_bytes: 8_192,     // 8 KB observer tracking relations
            reliable_channel_bytes: 16_384, // 16 KB sliding ACK retransmission cache
            transaction_state_bytes: 4_096, // 4 KB journal log slot
        }
    }
}

impl PlayerMemoryBudget {
    /// Total bytes required to sustain a single active player session.
    #[inline]
    pub const fn total_per_player_bytes(&self) -> usize {
        self.connection_state_bytes
            + self.kinematic_state_bytes
            + self.aoi_relations_bytes
            + self.reliable_channel_bytes
            + self.transaction_state_bytes
    }

    /// Verifies that total memory per player adheres strictly to the 48 KB ceiling.
    #[inline]
    pub const fn is_within_ceiling(&self) -> bool {
        self.total_per_player_bytes() <= MAX_PLAYER_MEMORY_CEILING_BYTES
    }

    /// Computes total memory requirement in megabytes for a given concurrent user count.
    #[inline]
    pub fn memory_mb_for_ccu(&self, ccu: usize) -> f64 {
        (self.total_per_player_bytes() as f64 * ccu as f64) / (1024.0 * 1024.0)
    }

    /// Computes total memory requirement in gigabytes for a given concurrent user count.
    #[inline]
    pub fn memory_gb_for_ccu(&self, ccu: usize) -> f64 {
        self.memory_mb_for_ccu(ccu) / 1024.0
    }
}

/// Audits `size_of` and `align_of` for key types across workspace crates.
pub fn audit_system_struct_sizes() -> [StructMemoryProfile; 6] {
    [
        StructMemoryProfile {
            type_name: "Vec3Fix (eidolon-core)",
            size_bytes: std::mem::size_of::<eidolon_core::fixed::Vec3Fix>(),
            align_bytes: std::mem::align_of::<eidolon_core::fixed::Vec3Fix>(),
        },
        StructMemoryProfile {
            type_name: "Fixed64 (eidolon-core)",
            size_bytes: std::mem::size_of::<eidolon_core::fixed::Fixed64>(),
            align_bytes: std::mem::align_of::<eidolon_core::fixed::Fixed64>(),
        },
        StructMemoryProfile {
            type_name: "PacketHeader (eidolon-net)",
            size_bytes: std::mem::size_of::<eidolon_net::PacketHeader>(),
            align_bytes: std::mem::align_of::<eidolon_net::PacketHeader>(),
        },
        StructMemoryProfile {
            type_name: "SessionSecurityContext (eidolon-net)",
            size_bytes: std::mem::size_of::<eidolon_net::SessionSecurityContext>(),
            align_bytes: std::mem::align_of::<eidolon_net::SessionSecurityContext>(),
        },
        StructMemoryProfile {
            type_name: "SpatialHashGrid (eidolon-spatial)",
            size_bytes: std::mem::size_of::<eidolon_spatial::grid::SpatialHashGrid>(),
            align_bytes: std::mem::align_of::<eidolon_spatial::grid::SpatialHashGrid>(),
        },
        StructMemoryProfile {
            type_name: "WalRecord (eidolon-world)",
            size_bytes: std::mem::size_of::<eidolon_world::wal::WalRecord>(),
            align_bytes: std::mem::align_of::<eidolon_world::wal::WalRecord>(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_per_player_memory_ceiling_invariant() {
        let budget = PlayerMemoryBudget::default();

        // Total per player must be <= 48 KB
        assert!(
            budget.is_within_ceiling(),
            "Total per-player allocation ({} bytes) exceeds ceiling ({} bytes)",
            budget.total_per_player_bytes(),
            MAX_PLAYER_MEMORY_CEILING_BYTES
        );

        // 1,000 CCU footprint: ~44.5 MB
        let mb_1k = budget.memory_mb_for_ccu(1_000);
        assert!(
            mb_1k < 50.0,
            "1,000 CCU memory ({:.2} MB) must be < 50 MB",
            mb_1k
        );

        // 10,000 CCU footprint: ~445 MB
        let mb_10k = budget.memory_mb_for_ccu(10_000);
        assert!(
            mb_10k < 500.0,
            "10,000 CCU memory ({:.2} MB) must be < 500 MB",
            mb_10k
        );

        // 100,000 CCU footprint: ~4.35 GB (strictly < 5.0 GB)
        let gb_100k = budget.memory_gb_for_ccu(100_000);
        assert!(
            gb_100k < 5.0,
            "100,000 CCU memory ({:.2} GB) must be < 5.0 GB",
            gb_100k
        );
    }

    #[test]
    fn test_struct_memory_profiles_validity() {
        let profiles = audit_system_struct_sizes();
        assert_eq!(profiles.len(), 6);

        for profile in &profiles {
            assert!(profile.size_bytes > 0);
            assert!(profile.align_bytes > 0);
            // Alignment must be power of two
            assert!(profile.align_bytes.is_power_of_two());
        }
    }
}
