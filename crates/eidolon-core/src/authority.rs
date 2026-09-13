//! Deterministic authority epochs, session fencing, and stale command rejection.
//!
//! Provides the mathematical foundation for multi-node authority tracking:
//! - 4-part authority tuple: `[account_id, session_id, authority_epoch, sequence]`.
//! - Monotonic epoch progression ensuring superseded server assignments cannot corrupt state.
//! - Reconnect fencing preventing race conditions during client cluster migration.

use core::fmt;

/// Wire length of serialized AuthorityToken in bytes: account (8) + session (8) + epoch (4) + sequence (4).
pub const AUTHORITY_TOKEN_LEN: usize = 24;

/// Errors resulting from authority validation and epoch fencing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityError {
    /// Packet epoch is older than the current authoritative server epoch.
    StaleEpoch {
        /// Current authoritative epoch.
        current: u32,
        /// Epoch claimed in packet.
        packet: u32,
    },
    /// Sequence number regressed within the current authority epoch.
    SequenceRegression {
        /// Highest sequence observed.
        current: u32,
        /// Sequence claimed in packet.
        packet: u32,
    },
    /// Provided buffer is shorter than AUTHORITY_TOKEN_LEN.
    InvalidLength,
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleEpoch { current, packet } => write!(
                f,
                "Stale authority epoch: packet epoch {packet} rejected by active epoch {current}"
            ),
            Self::SequenceRegression { current, packet } => write!(
                f,
                "Sequence regression: packet sequence {packet} <= current sequence {current}"
            ),
            Self::InvalidLength => write!(f, "Authority token buffer truncated or invalid length"),
        }
    }
}

impl std::error::Error for AuthorityError {}

/// Monotonic authority token governing client command authority over an entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuthorityToken {
    /// Unique persistent player account identifier.
    pub account_id: u64,
    /// Ephemeral session identifier assigned during handshake.
    pub session_id: u64,
    /// Monotonically incrementing server assignment generation.
    pub authority_epoch: u32,
    /// Command sequence number within the current epoch.
    pub sequence: u32,
}

impl AuthorityToken {
    /// Creates a new authority token.
    #[inline]
    pub const fn new(
        account_id: u64,
        session_id: u64,
        authority_epoch: u32,
        sequence: u32,
    ) -> Self {
        Self {
            account_id,
            session_id,
            authority_epoch,
            sequence,
        }
    }

    /// Serializes token into destination buffer.
    pub fn write_to(&self, out: &mut [u8]) -> Result<usize, AuthorityError> {
        if out.len() < AUTHORITY_TOKEN_LEN {
            return Err(AuthorityError::InvalidLength);
        }
        let (acc, rest) = out.split_at_mut(8);
        acc.copy_from_slice(&self.account_id.to_be_bytes());

        let (sess, rest2) = rest.split_at_mut(8);
        sess.copy_from_slice(&self.session_id.to_be_bytes());

        let (ep, seq) = rest2.split_at_mut(4);
        ep.copy_from_slice(&self.authority_epoch.to_be_bytes());
        if let Some(dst) = seq.get_mut(..4) {
            dst.copy_from_slice(&self.sequence.to_be_bytes());
        }
        Ok(AUTHORITY_TOKEN_LEN)
    }

    /// Deserializes token from byte slice with strict bounds checking.
    pub fn read_from(src: &[u8]) -> Result<Self, AuthorityError> {
        if src.len() < AUTHORITY_TOKEN_LEN {
            return Err(AuthorityError::InvalidLength);
        }
        let mut acc_buf = [0u8; 8];
        if let Some(s) = src.get(..8) {
            acc_buf.copy_from_slice(s);
        }
        let account_id = u64::from_be_bytes(acc_buf);

        let mut sess_buf = [0u8; 8];
        if let Some(s) = src.get(8..16) {
            sess_buf.copy_from_slice(s);
        }
        let session_id = u64::from_be_bytes(sess_buf);

        let mut ep_buf = [0u8; 4];
        if let Some(s) = src.get(16..20) {
            ep_buf.copy_from_slice(s);
        }
        let authority_epoch = u32::from_be_bytes(ep_buf);

        let mut seq_buf = [0u8; 4];
        if let Some(s) = src.get(20..24) {
            seq_buf.copy_from_slice(s);
        }
        let sequence = u32::from_be_bytes(seq_buf);

        Ok(Self {
            account_id,
            session_id,
            authority_epoch,
            sequence,
        })
    }
}

/// Server-side validator enforcing monotonic authority epochs and sequence progression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorityFencer {
    /// Active authority epoch for this session or entity.
    active_epoch: u32,
    /// Highest sequence processed in the active epoch.
    highest_sequence: u32,
}

impl Default for AuthorityFencer {
    fn default() -> Self {
        Self::new(1)
    }
}

impl AuthorityFencer {
    /// Creates a new authority fencer initialized to the specified epoch.
    pub const fn new(initial_epoch: u32) -> Self {
        Self {
            active_epoch: initial_epoch,
            highest_sequence: 0,
        }
    }

    /// Validates an incoming authority token and advances sequence state.
    ///
    /// - Rejects tokens with `epoch < active_epoch` (stale from old server/session).
    /// - Automatically promotes to newer epochs (`epoch > active_epoch`).
    /// - Enforces strictly increasing sequences within the same epoch.
    pub fn validate_and_advance(&mut self, token: &AuthorityToken) -> Result<(), AuthorityError> {
        if token.authority_epoch < self.active_epoch {
            return Err(AuthorityError::StaleEpoch {
                current: self.active_epoch,
                packet: token.authority_epoch,
            });
        }

        if token.authority_epoch > self.active_epoch {
            self.active_epoch = token.authority_epoch;
            self.highest_sequence = token.sequence;
            return Ok(());
        }

        // Same epoch: sequence must strictly progress
        if token.sequence <= self.highest_sequence {
            return Err(AuthorityError::SequenceRegression {
                current: self.highest_sequence,
                packet: token.sequence,
            });
        }

        self.highest_sequence = token.sequence;
        Ok(())
    }

    /// Manually increments the authority epoch (e.g. on client reconnect or zone failover).
    pub fn increment_epoch(&mut self) -> u32 {
        self.active_epoch = self.active_epoch.saturating_add(1);
        self.highest_sequence = 0;
        self.active_epoch
    }

    /// Returns the currently active authority epoch.
    #[inline]
    pub fn active_epoch(&self) -> u32 {
        self.active_epoch
    }

    /// Returns the highest sequence observed in the active epoch.
    #[inline]
    pub fn highest_sequence(&self) -> u32 {
        self.highest_sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_authority_token_roundtrip() {
        let token = AuthorityToken::new(49201, 88301, 3, 42);
        let mut buf = [0u8; AUTHORITY_TOKEN_LEN];
        assert_eq!(token.write_to(&mut buf).unwrap(), AUTHORITY_TOKEN_LEN);

        let parsed = AuthorityToken::read_from(&buf).unwrap();
        assert_eq!(token, parsed);
    }

    #[test]
    fn test_authority_fencer_in_order_sequences() {
        let mut fencer = AuthorityFencer::new(1);
        for seq in 1..=10 {
            let token = AuthorityToken::new(1, 1, 1, seq);
            assert!(fencer.validate_and_advance(&token).is_ok());
        }
        assert_eq!(fencer.highest_sequence(), 10);
    }

    #[test]
    fn test_authority_fencer_rejects_stale_epoch() {
        let mut fencer = AuthorityFencer::new(2);
        let stale_token = AuthorityToken::new(1, 1, 1, 50);
        assert_eq!(
            fencer.validate_and_advance(&stale_token),
            Err(AuthorityError::StaleEpoch {
                current: 2,
                packet: 1
            })
        );
    }

    #[test]
    fn test_authority_fencer_promotes_epoch() {
        let mut fencer = AuthorityFencer::new(1);
        let new_epoch_token = AuthorityToken::new(1, 1, 2, 1);
        assert!(fencer.validate_and_advance(&new_epoch_token).is_ok());
        assert_eq!(fencer.active_epoch(), 2);
        assert_eq!(fencer.highest_sequence(), 1);

        // Older sequence from epoch 1 now strictly rejected
        let old_token = AuthorityToken::new(1, 1, 1, 999);
        assert!(fencer.validate_and_advance(&old_token).is_err());
    }

    #[test]
    fn test_authority_fencer_rejects_sequence_regression() {
        let mut fencer = AuthorityFencer::new(1);
        let token1 = AuthorityToken::new(1, 1, 1, 5);
        assert!(fencer.validate_and_advance(&token1).is_ok());

        let token_dup = AuthorityToken::new(1, 1, 1, 5);
        assert!(fencer.validate_and_advance(&token_dup).is_err());

        let token_stale = AuthorityToken::new(1, 1, 1, 4);
        assert!(fencer.validate_and_advance(&token_stale).is_err());
    }
}
