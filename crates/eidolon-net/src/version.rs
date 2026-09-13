//! Wire protocol negotiation, backward-compatibility matrix, and semantic versioning.
//!
//! Enables zero-downtime rolling cluster upgrades by negotiating protocol capabilities
//! between mixed client and server versions.

use crate::error::NetError;
use crate::protocol::PROTOCOL_VERSION;

/// Minimum protocol version supported by this node.
pub const MIN_SUPPORTED_VERSION: u16 = 1;

/// Maximum protocol version supported by this node.
pub const MAX_SUPPORTED_VERSION: u16 = 2;

/// Protocol feature bitmask flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolFeatures(pub u32);

impl ProtocolFeatures {
    /// Baseline 20 Hz simulation with 12-byte header and 7-byte transform.
    pub const BASE_SIMULATION: u32 = 1 << 0;
    /// Dynamic 17-byte cell-anchor replication updates.
    pub const CELL_ANCHOR_REPLICATION: u32 = 1 << 1;
    /// Ephemeral HMAC session authentication handshake.
    pub const AUTHENTICATED_SESSIONS: u32 = 1 << 2;
    /// Extended 64-bit sequence replay window protection.
    pub const REPLAY_WINDOW_64BIT: u32 = 1 << 3;

    /// Standard features enabled in protocol version 1.
    pub const VERSION_1_FEATURES: u32 = Self::BASE_SIMULATION | Self::CELL_ANCHOR_REPLICATION;

    /// Standard features enabled in protocol version 2.
    pub const VERSION_2_FEATURES: u32 =
        Self::VERSION_1_FEATURES | Self::AUTHENTICATED_SESSIONS | Self::REPLAY_WINDOW_64BIT;

    /// Creates a feature set from raw bitmask.
    #[inline]
    pub const fn new(flags: u32) -> Self {
        Self(flags)
    }

    /// Checks if a specific feature flag is supported.
    #[inline]
    pub fn has_feature(&self, flag: u32) -> bool {
        (self.0 & flag) == flag
    }

    /// Computes the intersection of two feature sets.
    #[inline]
    pub fn intersect(&self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

/// Negotiator handling cross-version compatibility between client and server nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolNegotiator {
    server_version: u16,
    min_supported: u16,
    max_supported: u16,
}

impl Default for ProtocolNegotiator {
    fn default() -> Self {
        Self::new(
            PROTOCOL_VERSION,
            MIN_SUPPORTED_VERSION,
            MAX_SUPPORTED_VERSION,
        )
    }
}

impl ProtocolNegotiator {
    /// Creates a new protocol negotiator.
    pub fn new(server_version: u16, min_supported: u16, max_supported: u16) -> Self {
        Self {
            server_version,
            min_supported,
            max_supported,
        }
    }

    /// Negotiates an operating protocol version for an incoming client connection.
    ///
    /// Returns the agreed protocol version if compatible.
    /// Returns `Err(NetError::IncompatibleProtocolVersion)` if client version is outside supported bounds.
    pub fn negotiate(&self, client_version: u16) -> Result<u16, NetError> {
        if client_version < self.min_supported || client_version > self.max_supported {
            return Err(NetError::IncompatibleProtocolVersion {
                server_version: self.server_version,
                client_version,
            });
        }
        // Negotiate to the lowest common denominator for backward compatibility
        Ok(client_version.min(self.server_version))
    }

    /// Returns the supported features for a negotiated protocol version.
    pub fn features_for_version(&self, version: u16) -> ProtocolFeatures {
        match version {
            1 => ProtocolFeatures::new(ProtocolFeatures::VERSION_1_FEATURES),
            2 => ProtocolFeatures::new(ProtocolFeatures::VERSION_2_FEATURES),
            _ => ProtocolFeatures::new(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_negotiate_same_version() {
        let negotiator = ProtocolNegotiator::default();
        assert_eq!(negotiator.negotiate(1).unwrap(), 1);
    }

    #[test]
    fn test_negotiate_backward_compatibility() {
        let negotiator = ProtocolNegotiator::new(2, 1, 2);
        // Server v2 accepts Client v1 and downgrades session to v1
        assert_eq!(negotiator.negotiate(1).unwrap(), 1);
        // Server v2 accepts Client v2
        assert_eq!(negotiator.negotiate(2).unwrap(), 2);
    }

    #[test]
    fn test_negotiate_incompatible_version() {
        let negotiator = ProtocolNegotiator::new(1, 1, 1);
        // Client v0 rejected
        assert!(negotiator.negotiate(0).is_err());
        // Client v2 rejected by strict v1 server
        assert!(negotiator.negotiate(2).is_err());
    }

    #[test]
    fn test_feature_intersection() {
        let v1_feats = ProtocolFeatures::new(ProtocolFeatures::VERSION_1_FEATURES);
        let v2_feats = ProtocolFeatures::new(ProtocolFeatures::VERSION_2_FEATURES);

        assert!(v2_feats.has_feature(ProtocolFeatures::AUTHENTICATED_SESSIONS));
        assert!(!v1_feats.has_feature(ProtocolFeatures::AUTHENTICATED_SESSIONS));

        let common = v1_feats.intersect(v2_feats);
        assert_eq!(common, v1_feats);
    }
}
