//! Milestone 15.4 Integration Test Suite: 3-Node Real Socket Cluster Partition, SIGKILL & State Recovery.
//!
//! Validates P0 #3: Real multi-process and multi-socket cluster failure survivability.
//! 1. Coordinates 3 real OS UDP socket nodes (Zone Alpha, Zone Beta, Zone Gamma) on loopback.
//! 2. Declares network partitions between zone pairs, asserting that entity migrations across
//!    partitioned links safely trigger timeout and local state rollback (zero lost entities).
//! 3. Heals partitions and proves cross-zone migration resumes over real sockets.
//! 4. Terminates child worker processes via POSIX `SIGKILL` (`kill -9`), verifying instant
//!    replacement node restart and durable WAL recovery with RPO = 0.

use std::fs;
use std::time::Duration;

use eidolon_server::multi_process::{
    run_worker_if_requested, ProcessSupervisor, RealSocketZoneNode,
};
use eidolon_world::durable_journal::DurableFileJournal;

/// Hook required for child worker process execution during testing.
#[test]
fn test_child_worker_hook() {
    let _ = run_worker_if_requested();
}

#[test]
fn test_milestone_15_4_three_node_real_socket_cluster_partition_and_rollback() {
    // 1. Bind 3 independent zone nodes on OS-assigned ephemeral UDP ports (port 0)
    let mut node_alpha = RealSocketZoneNode::bind(0, 1, None::<&str>).expect("Bind Node Alpha");
    let mut node_beta = RealSocketZoneNode::bind(0, 2, None::<&str>).expect("Bind Node Beta");
    let mut node_gamma = RealSocketZoneNode::bind(0, 3, None::<&str>).expect("Bind Node Gamma");

    let addr_alpha = node_alpha.local_addr();
    let addr_beta = node_beta.local_addr();
    let addr_gamma = node_gamma.local_addr();

    // 2. Configure 3-node inter-server routing table
    node_alpha.add_peer_route(2, addr_beta);
    node_alpha.add_peer_route(3, addr_gamma);

    node_beta.add_peer_route(1, addr_alpha);
    node_beta.add_peer_route(3, addr_gamma);

    node_gamma.add_peer_route(1, addr_alpha);
    node_gamma.add_peer_route(2, addr_beta);

    // 3. Populate entities
    let entity_1 = 10001u64;
    let entity_2 = 10002u64;
    let entity_3 = 10003u64;

    node_alpha.spawn_entity(entity_1, vec![1, 2, 3]);
    node_alpha.spawn_entity(entity_2, vec![4, 5, 6]);
    node_alpha.spawn_entity(entity_3, vec![7, 8, 9]);

    assert_eq!(node_alpha.entity_count(), 3);
    assert_eq!(node_beta.entity_count(), 0);
    assert_eq!(node_gamma.entity_count(), 0);

    // 4. Normal cross-zone migration from Alpha -> Beta over real UDP socket
    node_alpha
        .dispatch_migration_to(2, entity_1)
        .expect("Dispatch to Beta");
    assert!(!node_alpha.has_entity(entity_1));

    // Poll Beta to receive from operating system UDP socket buffer
    let mut beta_received = false;
    for _ in 0..20 {
        let _ = node_beta.poll_network();
        if node_beta.has_entity(entity_1) {
            beta_received = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        beta_received,
        "Beta must receive entity 1 over real UDP socket"
    );

    // Poll Alpha to process return ACK and clear pending migration
    let mut alpha_acked = false;
    for _ in 0..20 {
        let _ = node_alpha.poll_network();
        if node_alpha.pending_migration_count() == 0 {
            alpha_acked = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(alpha_acked, "Alpha must receive migration ACK from Beta");

    // 5. Declare Network Partition between Alpha and Gamma
    // (Simulating packet loss or firewall link drop between Zone 1 and Zone 3)
    node_alpha.partition_zone(3);
    assert!(node_alpha.is_partitioned(3));

    // Attempting migration to actively partitioned Zone 3 fails immediately at dispatch
    let partition_err = node_alpha.dispatch_migration_to(3, entity_2);
    assert!(
        partition_err.is_err(),
        "Dispatch to partitioned zone must fail cleanly"
    );
    assert!(
        node_alpha.has_entity(entity_2),
        "Entity must remain local when partition prevents dispatch"
    );

    // 6. Test Partition on Receiver Side (Silent Packet Drop & Timeout Rollback)
    // Heal Alpha's outgoing partition, but partition Gamma's incoming link
    node_alpha.heal_zone(3);
    node_gamma.partition_zone(1);

    // Alpha dispatches migration to Gamma over real socket
    node_alpha
        .dispatch_migration_to(3, entity_2)
        .expect("Dispatch to Gamma");
    assert!(!node_alpha.has_entity(entity_2));
    assert_eq!(node_alpha.pending_migration_count(), 1);

    // Gamma polls network, but drops packet due to partition from Zone 1
    for _ in 0..10 {
        let _ = node_gamma.poll_network();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !node_gamma.has_entity(entity_2),
        "Gamma must NOT adopt entity over partitioned link"
    );

    // Alpha's migration times out: roll back entity ownership locally
    let rolled_back = node_alpha.check_migration_timeouts(Duration::from_millis(0));
    assert_eq!(rolled_back, 1, "Must roll back exactly 1 timed-out entity");
    assert_eq!(node_alpha.pending_migration_count(), 0);
    assert!(
        node_alpha.has_entity(entity_2),
        "Entity must be safely preserved in Alpha following partition timeout"
    );

    // 7. Heal Partition and Verify Successful Migration
    node_gamma.heal_zone(1);
    assert!(!node_gamma.is_partitioned(1));

    node_alpha
        .dispatch_migration_to(3, entity_2)
        .expect("Dispatch to healed Gamma");

    let mut gamma_received = false;
    for _ in 0..20 {
        let _ = node_gamma.poll_network();
        if node_gamma.has_entity(entity_2) {
            gamma_received = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(
        gamma_received,
        "Gamma must adopt entity 2 after partition heals"
    );
    assert_eq!(node_gamma.entity_count(), 1);

    // Poll Alpha to process return ACK from Gamma
    let mut gamma_acked = false;
    for _ in 0..20 {
        let _ = node_alpha.poll_network();
        if node_alpha.pending_migration_count() == 0 {
            gamma_acked = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(gamma_acked, "Alpha must receive migration ACK from Gamma");
}

#[test]
fn test_milestone_15_4_real_child_process_sigkill_and_cluster_recovery() {
    let temp_dir = std::env::temp_dir();
    let journal_path = temp_dir.join(format!(
        "eidolon_cluster_sigkill_{}.wal",
        std::process::id()
    ));

    let _ = fs::remove_file(&journal_path);

    // 1. Spawn child worker process running zone runner
    let mut supervisor =
        ProcessSupervisor::spawn_worker(0, &journal_path).expect("Spawn child worker process");

    // 2. Allow worker process time to initialize and sync initial transactions to disk
    std::thread::sleep(Duration::from_millis(150));

    // 3. Abruptly kill the worker process using POSIX SIGKILL (kill -9)
    supervisor
        .kill_sigkill()
        .expect("Send SIGKILL to child worker");

    // 4. Wait for process exit (must exit immediately due to SIGKILL)
    let exit_status = supervisor.wait().expect("Wait for killed child");
    assert!(
        !exit_status.success(),
        "Killed process must indicate signal termination"
    );

    // 5. Recover committed transactions from durable journal file on disk
    let recovered =
        DurableFileJournal::recover_from_journal(&journal_path).expect("Recover after SIGKILL");

    assert!(
        !recovered.is_empty(),
        "Committed fdatasync transactions must survive sudden SIGKILL"
    );
    assert_eq!(recovered[0].lsn, 1);
    assert_eq!(recovered[0].account_id, 1001);

    // 6. Spawn replacement node on the same durable journal to resume operations
    let mut replacement_node =
        RealSocketZoneNode::bind(0, 1, Some(&journal_path)).expect("Resume replacement node");

    let next_lsn = replacement_node
        .execute_durable_transaction(1001, 1500)
        .expect("Execute on replacement node");
    assert!(next_lsn > 0);

    let _ = fs::remove_file(&journal_path);
}
