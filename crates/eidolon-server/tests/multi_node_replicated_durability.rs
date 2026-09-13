//! Milestone 15.2 Integration Test Suite: Multi-Node Replicated Durability & Cross-AZ Disaster Failover.
//!
//! Validates P0 #1: Multi-machine and availability-zone (AZ) failure survivability.
//! Demonstrates that when a primary node or entire AZ abruptly dies, transactions committed
//! under `ReplicationMode::SynchronousQuorum` survive across the surviving replica ensemble
//! with RPO = 0, allowing an immediate standby node to promote itself to primary and resume operations.

use std::fs;
use std::path::PathBuf;

use eidolon_world::durable_journal::DurableFileJournal;
use eidolon_world::replicated_journal::{ReplicatedJournalSink, ReplicationMode};
use eidolon_world::transaction::{AccountState, TransactionManager};
use eidolon_world::wal::WalRecord;

fn temp_wal_path(prefix: &str, id: usize) -> PathBuf {
    let dir = std::env::temp_dir();
    dir.join(format!(
        "eidolon_az_failover_{}_{}_{}.wal",
        prefix,
        std::process::id(),
        id
    ))
}

#[test]
fn test_milestone_15_2_multi_node_quorum_durability_and_az_failover() {
    // Topology Setup: 3 Nodes across 3 simulated Availability Zones
    // AZ-1: Primary Storage Node
    // AZ-2: Standby Storage Node 1
    // AZ-3: Standby Storage Node 2
    let az1_primary_path = temp_wal_path("az1_primary", 1);
    let az2_replica_path = temp_wal_path("az2_replica", 2);
    let az3_replica_path = temp_wal_path("az3_replica", 3);
    let az2_promoted_path = temp_wal_path("az2_promoted", 4);

    let _ = fs::remove_file(&az1_primary_path);
    let _ = fs::remove_file(&az2_replica_path);
    let _ = fs::remove_file(&az3_replica_path);
    let _ = fs::remove_file(&az2_promoted_path);

    // 1. Initialize 3-Node Synchronous Quorum Ensemble (W=2 of N=3)
    let mut quorum_sink = ReplicatedJournalSink::new(
        &az1_primary_path,
        ReplicationMode::SynchronousQuorum {
            total_nodes: 3,
            required_acks: 2,
        },
    )
    .expect("Create primary quorum sink in AZ-1");

    quorum_sink
        .add_replica(&az2_replica_path)
        .expect("Add AZ-2 replica");
    quorum_sink
        .add_replica(&az3_replica_path)
        .expect("Add AZ-3 replica");

    assert_eq!(quorum_sink.replica_count(), 2);

    // 2. Execute Transactions on Primary Node
    let mut tx_manager = TransactionManager::new();
    let account_id = 99101u64;
    let mut account = AccountState::new(account_id);
    account.credit_currency(5000, true);
    tx_manager.register_account(account);

    let mut committed_lsns = Vec::new();

    // Commit 5 sequential currency transactions
    for lsn in 1..=5u64 {
        let mut payload = [0u8; 8];
        let amount = lsn * 100;
        payload.copy_from_slice(&amount.to_be_bytes());

        let record =
            WalRecord::new(lsn, lsn * 10, account_id, 1, 0x04, &payload).expect("Create WalRecord");

        let bytes_written = quorum_sink
            .append_and_replicate(&record)
            .expect("Replicate to quorum");
        assert!(bytes_written > 0);
        committed_lsns.push(lsn);
    }

    assert_eq!(quorum_sink.records_replicated(), 5);

    // 3. Simulate Complete AZ-1 Disaster / Primary Machine Death
    // AZ-1 completely disappears: primary storage node is dropped and corrupted/destroyed
    drop(quorum_sink);
    let _ = fs::remove_file(&az1_primary_path);

    // 4. Standby Promotion & Consensus Recovery in AZ-2
    // Surviving ensemble: AZ-2 and AZ-3
    let surviving_paths = [&az2_replica_path, &az3_replica_path];

    // Recover canonical state from quorum consensus across surviving nodes (W=2)
    let recovered_records = ReplicatedJournalSink::recover_quorum(&surviving_paths, 2)
        .expect("Recover quorum across surviving AZ replicas");

    // Invariant: RPO = 0 across complete machine or AZ loss
    assert_eq!(
        recovered_records.len(),
        5,
        "All 5 committed transactions must survive complete primary AZ destruction (RPO = 0)"
    );

    for (idx, rec) in recovered_records.iter().enumerate() {
        assert_eq!(rec.lsn, (idx + 1) as u64);
        assert_eq!(rec.account_id, account_id);
    }

    // 5. Promote AZ-2 Standby Replica to Primary Authority
    let mut promoted_primary =
        ReplicatedJournalSink::promote_to_primary(&az2_replica_path, &az2_promoted_path)
            .expect("Promote AZ-2 replica to primary");

    // 6. Resume Transactional Operations on New Primary in AZ-2
    let next_lsn = 6u64;
    let next_record = WalRecord::new(
        next_lsn,
        next_lsn * 10,
        account_id,
        1,
        0x04,
        &600u64.to_be_bytes(),
    )
    .expect("Next WalRecord");

    promoted_primary
        .append_and_sync(&next_record)
        .expect("Append on newly promoted primary");

    // Verify uninterrupted monotonic progression
    let final_records = DurableFileJournal::recover_from_journal(&az2_promoted_path)
        .expect("Recover from promoted primary");

    assert_eq!(final_records.len(), 6);
    assert_eq!(final_records[5].lsn, 6);

    // Cleanup temporary files
    let _ = fs::remove_file(&az1_primary_path);
    let _ = fs::remove_file(&az2_replica_path);
    let _ = fs::remove_file(&az3_replica_path);
    let _ = fs::remove_file(&az2_promoted_path);
}

#[test]
fn test_milestone_15_2_unacknowledged_transaction_discard_on_quorum_failure() {
    let az1_primary = temp_wal_path("az1_unack_prim", 10);
    let az2_replica = temp_wal_path("az2_unack_r1", 20);

    let _ = fs::remove_file(&az1_primary);
    let _ = fs::remove_file(&az2_replica);

    // Group requiring 3 acks, but only 2 nodes configured (Primary + 1 Replica)
    let mut sink = ReplicatedJournalSink::new(
        &az1_primary,
        ReplicationMode::SynchronousQuorum {
            total_nodes: 3,
            required_acks: 3,
        },
    )
    .expect("Create sink");

    sink.add_replica(&az2_replica).expect("Add replica");

    let record =
        WalRecord::new(1, 10, 77001, 1, 0x04, &500u64.to_be_bytes()).expect("Create WalRecord");

    // Commit fails because quorum (3) cannot be satisfied (achieved only 2)
    let res = sink.append_and_replicate(&record);
    assert!(
        res.is_err(),
        "Transaction must NOT be confirmed if quorum agreement fails"
    );

    // Quorum recovery requiring 3 acks correctly discards unacknowledged write
    let recovered =
        ReplicatedJournalSink::recover_quorum(&[&az1_primary, &az2_replica], 3).expect("Recover");
    assert!(
        recovered.is_empty(),
        "Unacknowledged transactions failing quorum must never be committed"
    );

    let _ = fs::remove_file(&az1_primary);
    let _ = fs::remove_file(&az2_replica);
}
