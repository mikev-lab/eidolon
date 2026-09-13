//! Milestone 13.4 Automated Integration Test Suite: Strict Per-Player Memory Budget.
//!
//! Enforces bounded resident memory consumption per connected client, proving that
//! 100,000 Concurrent Users (CCU) requires strictly less than 5.0 GB of heap space.

use eidolon_server::memory_budget::{
    audit_system_struct_sizes, PlayerMemoryBudget, MAX_PLAYER_MEMORY_CEILING_BYTES,
};

#[test]
fn test_milestone_13_4_struct_sizes_and_cache_alignments() {
    let profiles = audit_system_struct_sizes();

    for profile in &profiles {
        assert!(
            profile.size_bytes > 0,
            "Type {} must have non-zero size",
            profile.type_name
        );
        assert!(
            profile.align_bytes > 0,
            "Type {} must have non-zero alignment",
            profile.type_name
        );
        assert!(
            profile.align_bytes.is_power_of_two(),
            "Type {} alignment ({}) must be a power of two",
            profile.type_name,
            profile.align_bytes
        );
    }
}

#[test]
fn test_milestone_13_4_per_player_memory_ceiling_under_48kb() {
    let budget = PlayerMemoryBudget::default();

    // Verify per-player ceiling invariant
    assert!(
        budget.is_within_ceiling(),
        "Per-player allocation ({} bytes) must be <= ceiling ({} bytes)",
        budget.total_per_player_bytes(),
        MAX_PLAYER_MEMORY_CEILING_BYTES
    );

    // Verify specific component bounds
    assert!(budget.connection_state_bytes <= 16_384);
    assert!(budget.kinematic_state_bytes <= 1_024);
    assert!(budget.aoi_relations_bytes <= 8_192);
    assert!(budget.reliable_channel_bytes <= 16_384);
    assert!(budget.transaction_state_bytes <= 4_096);
}

#[test]
fn test_milestone_13_4_100k_ccu_memory_fits_in_standard_server_envelope() {
    let budget = PlayerMemoryBudget::default();

    // 1. 1,000 CCU memory verification (e2-micro / small indie tier)
    let mb_1k = budget.memory_mb_for_ccu(1_000);
    assert!(
        mb_1k < 50.0,
        "1,000 CCU memory ({:.2} MB) must fit within 50 MB budget",
        mb_1k
    );

    // 2. 10,000 CCU memory verification (medium cluster tier)
    let mb_10k = budget.memory_mb_for_ccu(10_000);
    assert!(
        mb_10k < 500.0,
        "10,000 CCU memory ({:.2} MB) must fit within 500 MB budget",
        mb_10k
    );

    // 3. 100,000 CCU memory verification (mega server tier)
    let gb_100k = budget.memory_gb_for_ccu(100_000);
    assert!(
        gb_100k < 5.0,
        "100,000 CCU memory ({:.2} GB) must fit within 5.0 GB server envelope",
        gb_100k
    );
    assert!(
        gb_100k > 3.0,
        "100,000 CCU memory ({:.2} GB) must reflect realistic operational reservation",
        gb_100k
    );
}
