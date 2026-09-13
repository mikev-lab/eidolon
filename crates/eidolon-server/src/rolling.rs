//! Zero-downtime rolling cluster upgrade simulator and wire translation coordinator.
//!
//! Orchestrates live player migration between mixed-version cluster nodes (Version N and Version N+1)
//! verifying backward wire protocol compatibility, state preservation, and zero client disconnects.

use eidolon_net::version::{ProtocolFeatures, ProtocolNegotiator};
use eidolon_world::error::WorldError;

/// Client connection descriptor tracked during rolling upgrades.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpgradeSession {
    /// Client session identifier.
    pub session_id: u32,
    /// Client wire protocol version.
    pub client_version: u16,
    /// Current assigned cluster node identifier.
    pub assigned_node: u32,
    /// Negotiated feature flags.
    pub negotiated_features: ProtocolFeatures,
    /// Monotonic migration counter across rolling upgrade steps.
    pub migrations_count: u32,
}

/// Simulated node within a mixed-version rolling cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollingClusterNode {
    /// Node identifier.
    pub node_id: u32,
    /// Active protocol version deployed on this node.
    pub protocol_version: u16,
    /// Total connected sessions currently situated on this node.
    pub active_sessions: usize,
}

/// Zero-downtime rolling upgrade orchestrator.
#[derive(Debug)]
pub struct RollingUpgradeSimulator {
    nodes: [Option<RollingClusterNode>; 8],
    node_count: usize,
    sessions: [Option<UpgradeSession>; 128],
    session_count: usize,
    negotiator: ProtocolNegotiator,
}

impl RollingUpgradeSimulator {
    /// Creates a new rolling upgrade simulator.
    pub fn new() -> Self {
        Self {
            nodes: [None; 8],
            node_count: 0,
            sessions: [None; 128],
            session_count: 0,
            negotiator: ProtocolNegotiator::new(2, 1, 2),
        }
    }

    /// Returns the active cluster protocol negotiator.
    #[inline]
    pub const fn negotiator(&self) -> &ProtocolNegotiator {
        &self.negotiator
    }

    /// Registers a server node with its deployed protocol version into the rolling cluster.
    pub fn add_node(&mut self, node_id: u32, protocol_version: u16) -> Result<(), WorldError> {
        if self.node_count >= self.nodes.len() {
            return Err(WorldError::TransactionAborted("node_capacity_reached"));
        }

        for slot in self.nodes.iter_mut() {
            if slot.is_none() {
                *slot = Some(RollingClusterNode {
                    node_id,
                    protocol_version,
                    active_sessions: 0,
                });
                self.node_count += 1;
                return Ok(());
            }
        }
        Err(WorldError::TransactionAborted("no_node_slot"))
    }

    /// Connects a client session to an initial cluster node, negotiating protocol capabilities.
    pub fn connect_session(
        &mut self,
        session_id: u32,
        client_version: u16,
        node_id: u32,
    ) -> Result<ProtocolFeatures, WorldError> {
        let node_version = self
            .get_node_version(node_id)
            .ok_or(WorldError::TransactionAborted("node_not_found"))?;

        let node_negotiator = ProtocolNegotiator::new(node_version, 1, 2);
        let agreed_version = node_negotiator
            .negotiate(client_version)
            .map_err(|_| WorldError::TransactionAborted("incompatible_version"))?;

        let agreed_features = node_negotiator.features_for_version(agreed_version);

        for slot in self.sessions.iter_mut() {
            if slot.is_none() {
                *slot = Some(UpgradeSession {
                    session_id,
                    client_version,
                    assigned_node: node_id,
                    negotiated_features: agreed_features,
                    migrations_count: 0,
                });
                self.session_count += 1;
                self.increment_node_sessions(node_id);
                return Ok(agreed_features);
            }
        }

        Err(WorldError::TransactionAborted("session_capacity_reached"))
    }

    /// Seamlessly migrates an active session to a different node (e.g. during pod replacement or rolling drain).
    pub fn live_migrate_session(
        &mut self,
        session_id: u32,
        target_node_id: u32,
    ) -> Result<(), WorldError> {
        let target_node_version = self
            .get_node_version(target_node_id)
            .ok_or(WorldError::TransactionAborted("target_node_not_found"))?;

        let session = self
            .find_session_mut(session_id)
            .ok_or(WorldError::TransactionAborted("session_not_found"))?;

        let old_node_id = session.assigned_node;

        // Re-negotiate compatibility with target node without socket disconnect
        let target_negotiator = ProtocolNegotiator::new(target_node_version, 1, 2);
        let agreed_version = target_negotiator
            .negotiate(session.client_version)
            .map_err(|_| WorldError::TransactionAborted("target_version_incompatible"))?;

        session.assigned_node = target_node_id;
        session.negotiated_features = target_negotiator.features_for_version(agreed_version);
        session.migrations_count += 1;

        self.decrement_node_sessions(old_node_id);
        self.increment_node_sessions(target_node_id);

        Ok(())
    }

    /// Returns the active protocol version for a given node.
    pub fn get_node_version(&self, node_id: u32) -> Option<u16> {
        self.nodes
            .iter()
            .flatten()
            .find(|n| n.node_id == node_id)
            .map(|n| n.protocol_version)
    }

    /// Returns session information for diagnosis.
    pub fn get_session(&self, session_id: u32) -> Option<&UpgradeSession> {
        self.sessions
            .iter()
            .flatten()
            .find(|s| s.session_id == session_id)
    }

    fn find_session_mut(&mut self, session_id: u32) -> Option<&mut UpgradeSession> {
        self.sessions
            .iter_mut()
            .flatten()
            .find(|sess| sess.session_id == session_id)
    }

    fn increment_node_sessions(&mut self, node_id: u32) {
        for node in self.nodes.iter_mut().flatten() {
            if node.node_id == node_id {
                node.active_sessions += 1;
                break;
            }
        }
    }

    fn decrement_node_sessions(&mut self, node_id: u32) {
        for node in self.nodes.iter_mut().flatten() {
            if node.node_id == node_id {
                node.active_sessions = node.active_sessions.saturating_sub(1);
                break;
            }
        }
    }
}

impl Default for RollingUpgradeSimulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rolling_upgrade_migration_preserves_session() {
        let mut sim = RollingUpgradeSimulator::new();

        // Register Node 1 (Version 1) and Node 2 (Version 2)
        assert!(sim.add_node(101, 1).is_ok());
        assert!(sim.add_node(102, 2).is_ok());

        // Connect Client running Version 1 to Node 1
        let agreed = sim.connect_session(5001, 1, 101).unwrap();
        assert!(agreed.has_feature(ProtocolFeatures::BASE_SIMULATION));

        let sess = sim.get_session(5001).unwrap();
        assert_eq!(sess.assigned_node, 101);
        assert_eq!(sess.migrations_count, 0);

        // Perform live migration from Node 1 (v1) to Node 2 (v2)
        assert!(sim.live_migrate_session(5001, 102).is_ok());

        // Session must be intact with incremented migration count and zero disconnects
        let updated_sess = sim.get_session(5001).unwrap();
        assert_eq!(updated_sess.assigned_node, 102);
        assert_eq!(updated_sess.migrations_count, 1);
        assert!(updated_sess
            .negotiated_features
            .has_feature(ProtocolFeatures::BASE_SIMULATION));
    }
}
