//! Automated test suite verifying Milestone 8.5: Wire Protocol Versioning & Rolling Compatibility.
//!
//! Validates:
//! - Runtime protocol version negotiation between heterogeneous client and server versions.
//! - Backward-compatible feature set negotiation enabling zero-downtime rolling upgrades.
//! - Strict typed error rejection for unsupported legacy or future wire protocol versions.

use eidolon_net::error::NetError;
use eidolon_net::version::{ProtocolFeatures, ProtocolNegotiator};

#[test]
fn test_milestone_8_5_protocol_negotiation_matrix() {
    // Scenario 1: Standard v1 client connecting to v1 server
    let server_v1 = ProtocolNegotiator::new(1, 1, 1);
    assert_eq!(server_v1.negotiate(1).unwrap(), 1);

    // Scenario 2: Rolling upgrade: v2 server accepts legacy v1 client
    let server_v2 = ProtocolNegotiator::new(2, 1, 2);
    let negotiated_v1 = server_v2.negotiate(1).unwrap();
    assert_eq!(
        negotiated_v1, 1,
        "Server v2 must negotiate to v1 for legacy client"
    );

    // Features for v1 client on v2 server downgrade gracefully
    let features_v1 = server_v2.features_for_version(negotiated_v1);
    assert!(features_v1.has_feature(ProtocolFeatures::BASE_SIMULATION));
    assert!(features_v1.has_feature(ProtocolFeatures::CELL_ANCHOR_REPLICATION));
    assert!(!features_v1.has_feature(ProtocolFeatures::AUTHENTICATED_SESSIONS));

    // Scenario 3: v2 client connecting to v2 server enjoys full feature set
    let negotiated_v2 = server_v2.negotiate(2).unwrap();
    assert_eq!(negotiated_v2, 2);
    let features_v2 = server_v2.features_for_version(negotiated_v2);
    assert!(features_v2.has_feature(ProtocolFeatures::AUTHENTICATED_SESSIONS));
    assert!(features_v2.has_feature(ProtocolFeatures::REPLAY_WINDOW_64BIT));

    // Scenario 4: Incompatible future client (v3) rejected by v2 server
    assert_eq!(
        server_v2.negotiate(3),
        Err(NetError::IncompatibleProtocolVersion {
            server_version: 2,
            client_version: 3
        })
    );

    // Scenario 5: Incompatible legacy client (v0) rejected
    assert_eq!(
        server_v2.negotiate(0),
        Err(NetError::IncompatibleProtocolVersion {
            server_version: 2,
            client_version: 0
        })
    );
}

#[test]
fn test_milestone_8_5_rolling_cluster_downgrade_simulation() {
    // Cluster consists of 2 nodes: Node A (v1) and Node B (v2) during rolling rollout
    let node_a = ProtocolNegotiator::new(1, 1, 1);
    let node_b = ProtocolNegotiator::new(2, 1, 2);

    let client_versions = [1u16, 2u16];

    for &c_ver in &client_versions {
        let node_a_res = node_a.negotiate(c_ver);
        let node_b_res = node_b.negotiate(c_ver);

        if c_ver == 1 {
            // Both nodes support v1 client seamlessly
            assert_eq!(node_a_res.unwrap(), 1);
            assert_eq!(node_b_res.unwrap(), 1);
        } else if c_ver == 2 {
            // Node A rejects v2 until upgraded; Node B supports v2
            assert!(node_a_res.is_err());
            assert_eq!(node_b_res.unwrap(), 2);
        }
    }
}
