//! Milestone 12.5 Automated Integration Test Suite: Zero-Downtime Rolling Upgrades.
//!
//! Validates mixed-version cluster execution (Version N and Version N+1) and live session
//! migrations across versioned boundaries without player disconnects.

use eidolon_net::version::ProtocolFeatures;
use eidolon_server::rolling::RollingUpgradeSimulator;

#[test]
fn test_milestone_12_5_mixed_version_cluster_live_migration() {
    let mut cluster = RollingUpgradeSimulator::new();

    // Node 1 & 2: Version 1 (legacy nodes)
    assert!(cluster.add_node(1, 1).is_ok());
    assert!(cluster.add_node(2, 1).is_ok());

    // Node 3 & 4: Version 2 (upgraded nodes)
    assert!(cluster.add_node(3, 2).is_ok());
    assert!(cluster.add_node(4, 2).is_ok());

    // Connect 20 Version 1 clients to Node 1
    for session_id in 1..=20 {
        let agreed = cluster.connect_session(session_id, 1, 1).unwrap();
        assert!(agreed.has_feature(ProtocolFeatures::BASE_SIMULATION));
    }

    // Connect 20 Version 2 clients to Node 3
    for session_id in 21..=40 {
        let agreed = cluster.connect_session(session_id, 2, 3).unwrap();
        assert!(agreed.has_feature(ProtocolFeatures::BASE_SIMULATION));
    }

    // Drain Node 1: live-migrate all 20 Version 1 clients to upgraded Node 3
    for session_id in 1..=20 {
        assert!(cluster.live_migrate_session(session_id, 3).is_ok());
        let session = cluster.get_session(session_id).unwrap();
        assert_eq!(session.assigned_node, 3);
        assert_eq!(session.migrations_count, 1);
        // Backward compatibility preserved on upgraded node
        assert!(session
            .negotiated_features
            .has_feature(ProtocolFeatures::BASE_SIMULATION));
    }

    // Migrate Version 2 clients between upgraded nodes (Node 3 -> Node 4)
    for session_id in 21..=40 {
        assert!(cluster.live_migrate_session(session_id, 4).is_ok());
        let session = cluster.get_session(session_id).unwrap();
        assert_eq!(session.assigned_node, 4);
        assert_eq!(session.migrations_count, 1);
    }
}
