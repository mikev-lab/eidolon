//! Companion ecosystem integration for archive-aware delta patching.
//!
//! Provides asset version negotiation, manifest verification, and delta patch
//! synchronization state machines designed to work in tandem with `pak-delta`.

use crate::error::ZoneId;

/// Fixed 16-byte cryptographic or Adler/BLAKE3 digest identifying asset container state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct AssetManifestDigest(pub [u8; 16]);

impl AssetManifestDigest {
    /// Creates a new asset manifest digest from raw 16 bytes.
    #[inline]
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Constructs a synthetic digest from a 32-bit version number for testing and simulation.
    #[inline]
    pub const fn from_version(version: u32) -> Self {
        let b = version.to_le_bytes();
        Self([
            b[0], b[1], b[2], b[3], 0x45, 0x49, 0x50, 0x4B, // "EIPK"
            0, 0, 0, 0, 0, 0, 0, 0,
        ])
    }

    /// Returns byte slice representation of the digest.
    #[inline]
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Returns true if this digest matches the remote target digest.
    #[inline]
    pub fn matches(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

/// Asset requirement specification for a zone, dungeon, or live-service event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneAssetRequirement {
    /// Target zone or dungeon identifier.
    pub zone_id: ZoneId,
    /// Unique asset container or bundle identifier.
    pub bundle_id: u32,
    /// Required target asset container digest.
    pub required_digest: AssetManifestDigest,
    /// Expected byte size of the pak-delta differential patch if client is outdated.
    pub estimated_delta_bytes: u32,
}

impl ZoneAssetRequirement {
    /// Creates a new zone asset requirement.
    pub const fn new(
        zone_id: ZoneId,
        bundle_id: u32,
        required_digest: AssetManifestDigest,
        estimated_delta_bytes: u32,
    ) -> Self {
        Self {
            zone_id,
            bundle_id,
            required_digest,
            estimated_delta_bytes,
        }
    }
}

/// Lifecycle state machine for client companion asset synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchNegotiationState {
    /// Evaluating client's local asset container version.
    CheckingVersion,
    /// Client requires a pak-delta byte-level micro-patch before entry.
    DeltaRequired {
        /// Required manifest digest after patch application.
        target_digest: AssetManifestDigest,
        /// Differential patch download byte payload.
        delta_bytes: u32,
    },
    /// Client is actively applying the archive-aware byte delta.
    ApplyingPatch {
        /// Patch application progress percentage (0..=100).
        progress_pct: u8,
    },
    /// Asset container verified and synchronized with server state.
    Synchronized,
}

/// Negotiator managing the transition from asset patching to active gameplay replication.
#[derive(Debug, Clone, Copy)]
pub struct PatchNegotiator {
    requirement: ZoneAssetRequirement,
    state: PatchNegotiationState,
}

impl PatchNegotiator {
    /// Constructs a new patch negotiator for the specified zone requirement.
    pub fn new(requirement: ZoneAssetRequirement) -> Self {
        Self {
            requirement,
            state: PatchNegotiationState::CheckingVersion,
        }
    }

    /// Returns the active negotiation lifecycle state.
    #[inline]
    pub fn state(&self) -> PatchNegotiationState {
        self.state
    }

    /// Returns true if the client assets are fully synchronized.
    #[inline]
    pub fn is_synchronized(&self) -> bool {
        self.state == PatchNegotiationState::Synchronized
    }

    /// Evaluates the client's reported asset container digest.
    ///
    /// If client digest matches the requirement, transitions immediately to Synchronized.
    /// Otherwise, transitions to DeltaRequired and returns false.
    pub fn evaluate_client_manifest(&mut self, client_digest: AssetManifestDigest) -> bool {
        if self.requirement.required_digest.matches(&client_digest) {
            self.state = PatchNegotiationState::Synchronized;
            true
        } else {
            self.state = PatchNegotiationState::DeltaRequired {
                target_digest: self.requirement.required_digest,
                delta_bytes: self.requirement.estimated_delta_bytes,
            };
            false
        }
    }

    /// Reports patch application progress from the client.
    pub fn update_progress(&mut self, progress_pct: u8) -> Result<(), &'static str> {
        match self.state {
            PatchNegotiationState::DeltaRequired { .. }
            | PatchNegotiationState::ApplyingPatch { .. } => {
                let clamped = progress_pct.min(100);
                self.state = PatchNegotiationState::ApplyingPatch {
                    progress_pct: clamped,
                };
                Ok(())
            }
            _ => Err("Invalid state transition for patch progress update"),
        }
    }

    /// Completes patch application by verifying the newly patched asset digest.
    pub fn complete_patch(
        &mut self,
        new_client_digest: AssetManifestDigest,
    ) -> Result<bool, &'static str> {
        if !self.requirement.required_digest.matches(&new_client_digest) {
            return Ok(false);
        }

        self.state = PatchNegotiationState::Synchronized;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_companion_patch_negotiation_happy_path() {
        let digest_v1 = AssetManifestDigest::from_version(100);
        let req = ZoneAssetRequirement::new(ZoneId(1), 42, digest_v1, 0);
        let mut negotiator = PatchNegotiator::new(req);

        assert_eq!(negotiator.state(), PatchNegotiationState::CheckingVersion);
        assert!(!negotiator.is_synchronized());

        // Client presents matching version 100
        assert!(negotiator.evaluate_client_manifest(digest_v1));
        assert!(negotiator.is_synchronized());
    }

    #[test]
    fn test_companion_patch_delta_negotiation_flow() {
        let digest_v1 = AssetManifestDigest::from_version(100);
        let digest_v2 = AssetManifestDigest::from_version(101);

        let req = ZoneAssetRequirement::new(ZoneId(2), 55, digest_v2, 10240);
        let mut negotiator = PatchNegotiator::new(req);

        // Client presents outdated version 100
        assert!(!negotiator.evaluate_client_manifest(digest_v1));
        match negotiator.state() {
            PatchNegotiationState::DeltaRequired {
                target_digest,
                delta_bytes,
            } => {
                assert_eq!(target_digest, digest_v2);
                assert_eq!(delta_bytes, 10240);
            }
            _ => panic!("Expected DeltaRequired state"),
        }

        // Progress updates
        assert!(negotiator.update_progress(50).is_ok());
        assert_eq!(
            negotiator.state(),
            PatchNegotiationState::ApplyingPatch { progress_pct: 50 }
        );

        // Complete with wrong digest fails
        let wrong_digest = AssetManifestDigest::from_version(999);
        assert!(!negotiator.complete_patch(wrong_digest).expect("verify"));
        assert!(!negotiator.is_synchronized());

        // Complete with correct v2 digest succeeds
        assert!(negotiator.complete_patch(digest_v2).expect("verify"));
        assert!(negotiator.is_synchronized());
    }
}
