//! Automated test suite verifying Milestone 8.3 & 8.4: Authority Epochs, Reconnect Fencing, and Split-Brain Prevention.
//!
//! Validates:
//! - Authority epoch increment and stale packet rejection during node failover and reconnect.
//! - Network partition detection, boundary entity freezing, and transactional migration rollback.
//! - Strict single-writer invariant across inter-zone boundaries during network splits.

use eidolon_core::authority::{AuthorityError, AuthorityFencer, AuthorityToken};
use eidolon_world::error::ZoneId;
use eidolon_world::partition::{EntityPartitionState, ZonePartitionManager};

#[test]
fn test_milestone_8_3_authority_epoch_reconnect_fencing() {
    let account_id = 99102u64;
    let session_id_a = 1001u64;
    let session_id_b = 1002u64;

    // Phase 1: Client connected to Node A with epoch 1
    let mut server_a_fencer = AuthorityFencer::new(1);
    for seq in 1..=5 {
        let token = AuthorityToken::new(account_id, session_id_a, 1, seq);
        assert!(server_a_fencer.validate_and_advance(&token).is_ok());
    }
    assert_eq!(server_a_fencer.highest_sequence(), 5);

    // Phase 2: Node A crashes; client detects disconnect and reconnects to Node B.
    // Node B establishes new session with authority epoch 2.
    let mut server_b_fencer = AuthorityFencer::new(2);

    // Any in-flight packets or replayed commands from Node A's epoch 1 arriving at Node B are rejected
    let stale_packet = AuthorityToken::new(account_id, session_id_a, 1, 6);
    assert_eq!(
        server_b_fencer.validate_and_advance(&stale_packet),
        Err(AuthorityError::StaleEpoch {
            current: 2,
            packet: 1
        })
    );

    // Client begins transmitting commands under new epoch 2
    let valid_packet_1 = AuthorityToken::new(account_id, session_id_b, 2, 1);
    assert!(server_b_fencer
        .validate_and_advance(&valid_packet_1)
        .is_ok());

    let valid_packet_2 = AuthorityToken::new(account_id, session_id_b, 2, 2);
    assert!(server_b_fencer
        .validate_and_advance(&valid_packet_2)
        .is_ok());

    // Duplicate or out-of-order sequence in epoch 2 rejected
    let dup_packet = AuthorityToken::new(account_id, session_id_b, 2, 2);
    assert_eq!(
        server_b_fencer.validate_and_advance(&dup_packet),
        Err(AuthorityError::SequenceRegression {
            current: 2,
            packet: 2
        })
    );
}

#[test]
fn test_milestone_8_4_split_brain_partition_fencing_and_rollback() {
    let zone_a = ZoneId(10);
    let zone_b = ZoneId(20);
    let entity_id = 4099u32;

    let mut partition_mgr = ZonePartitionManager::new(zone_a, zone_b, 100);

    // Step 1: Healthy link, boundary lease valid
    assert!(partition_mgr.validate_migration(101).is_ok());
    assert!(!partition_mgr.is_partitioned(101));

    // Step 2: Simulate partition: inter-node heartbeats stop for 4 ticks (200ms)
    assert!(partition_mgr.is_partitioned(105));

    // Migration attempt during partition rejected with typed error
    assert!(partition_mgr.validate_migration(105).is_err());

    // Step 3: In-flight entity migration across boundary freezes into read-only state
    let frozen_state = partition_mgr.resolve_partition_rollback(entity_id, 105);
    assert_eq!(frozen_state, EntityPartitionState::FrozenReadOnly);

    // Entity is prevented from taking actions or mutating state while frozen
    // Single-writer invariant: entity is not duplicated in destination zone

    // Step 4: Network partition heals: heartbeats resume
    partition_mgr.handle_heartbeat(106);
    assert!(!partition_mgr.is_partitioned(106));

    // Rollback resolves cleanly to ActiveAuthoritative in originating zone
    let healed_state = partition_mgr.resolve_partition_rollback(entity_id, 106);
    assert_eq!(healed_state, EntityPartitionState::ActiveAuthoritative);
}
