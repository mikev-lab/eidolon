//! Multi-channel chat messaging, spatial proximity filtering, and anti-spam rate limiting.
//!
//! Enforces server-authoritative routing across spatial proximity, party, whisper,
//! and global channels with per-peer token-bucket rate policers.

use std::collections::HashMap;

use eidolon_core::fixed::{Fixed64, Vec3Fix};

use crate::error::WorldError;

/// Standard spatial proximity broadcast radius in meters (25.0m).
pub const DEFAULT_PROXIMITY_RADIUS_METERS: f64 = 25.0;

/// Chat channel communication scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ChatChannel {
    /// Local spatial proximity: broadcast exclusively to entities within 25 meters.
    SpatialProximity = 0,
    /// Party channel: delivered to all authenticated members of the sender's party.
    Party = 1,
    /// Direct whisper: private communication targeting a single character.
    Whisper = 2,
    /// Global zone broadcast: distributed to all active players within the zone.
    GlobalShout = 3,
}

impl ChatChannel {
    /// Parses chat channel from discrete u8 opcode.
    pub const fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::SpatialProximity),
            1 => Some(Self::Party),
            2 => Some(Self::Whisper),
            3 => Some(Self::GlobalShout),
            _ => None,
        }
    }

    /// Returns the raw u8 channel identifier.
    #[inline]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Validated chat message ready for transport routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    /// Target channel scope.
    pub channel: ChatChannel,
    /// Authoritative entity ID of the sender.
    pub sender_id: u32,
    /// Display name of the sender.
    pub sender_name: String,
    /// Optional target recipient entity ID (for whispers).
    pub recipient_id: Option<u32>,
    /// Global spatial coordinates of the sender (for proximity distance validation).
    pub sender_pos: Vec3Fix,
    /// Text message payload (clamped to max length, e.g. 256 characters).
    pub text: String,
    /// Simulation tick when the message was accepted.
    pub timestamp_tick: u64,
}

/// Token bucket entry for tracking per-account chat frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TokenBucket {
    tokens: u32,
    last_replenish_tick: u64,
}

/// Per-player chat anti-spam rate limiter.
///
/// Grants up to `capacity` burst messages and replenishes 1 token every `replenish_interval_ticks`.
#[derive(Debug, Clone)]
pub struct ChatRateLimiter {
    capacity: u32,
    replenish_interval_ticks: u64,
    buckets: HashMap<u64, TokenBucket>,
}

impl ChatRateLimiter {
    /// Constructs a new chat rate limiter.
    ///
    /// Default: 5 burst messages, 1 token refilled per 20 ticks (1.0 second).
    pub fn new(capacity: u32, replenish_interval_ticks: u64) -> Self {
        Self {
            capacity,
            replenish_interval_ticks: replenish_interval_ticks.max(1),
            buckets: HashMap::new(),
        }
    }

    /// Verifies if an account is permitted to send a message, consuming 1 token.
    ///
    /// Returns `Ok(())` on success, or `Err(WorldError::TransactionAborted("Chat rate limit exceeded"))` if empty.
    pub fn check_and_consume(
        &mut self,
        account_id: u64,
        current_tick: u64,
    ) -> Result<(), WorldError> {
        let bucket = self.buckets.entry(account_id).or_insert(TokenBucket {
            tokens: self.capacity,
            last_replenish_tick: current_tick,
        });

        // Replenish tokens based on elapsed simulation ticks
        if current_tick > bucket.last_replenish_tick {
            let elapsed = current_tick - bucket.last_replenish_tick;
            let added_tokens = (elapsed / self.replenish_interval_ticks) as u32;
            if added_tokens > 0 {
                bucket.tokens = (bucket.tokens + added_tokens).min(self.capacity);
                bucket.last_replenish_tick += (added_tokens as u64) * self.replenish_interval_ticks;
            }
        }

        if bucket.tokens == 0 {
            return Err(WorldError::TransactionAborted("Chat rate limit exceeded"));
        }

        bucket.tokens -= 1;
        Ok(())
    }
}

impl Default for ChatRateLimiter {
    fn default() -> Self {
        Self::new(5, 20) // 5 tokens, 1 replenished per second (20 ticks)
    }
}

/// Helper for validating whether a receiver is within spatial proximity.
pub fn is_within_proximity(sender_pos: Vec3Fix, receiver_pos: Vec3Fix, radius_meters: f64) -> bool {
    let diff = receiver_pos - sender_pos;
    let dist_sq = diff.magnitude_squared();
    let max_dist_sq = Fixed64::from_f64(radius_meters * radius_meters);
    dist_sq <= max_dist_sq
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_channel_opcodes() {
        for i in 0..=3 {
            let ch = ChatChannel::from_u8(i).expect("Valid channel");
            assert_eq!(ch.as_u8(), i);
        }
        assert_eq!(ChatChannel::from_u8(4), None);
    }

    #[test]
    fn test_proximity_distance_filtering() {
        let sender = Vec3Fix::from_f64(100.0, 0.0, 100.0);

        // Within 25m (distance = 15m)
        let close_receiver = Vec3Fix::from_f64(115.0, 0.0, 100.0);
        assert!(is_within_proximity(sender, close_receiver, 25.0));

        // Exactly 25m
        let boundary_receiver = Vec3Fix::from_f64(125.0, 0.0, 100.0);
        assert!(is_within_proximity(sender, boundary_receiver, 25.0));

        // Outside 25m (distance = 26m)
        let far_receiver = Vec3Fix::from_f64(126.0, 0.0, 100.0);
        assert!(!is_within_proximity(sender, far_receiver, 25.0));
    }

    #[test]
    fn test_chat_rate_limiter() {
        let mut limiter = ChatRateLimiter::new(3, 20); // 3 tokens, 1/sec
        let account = 1001;

        // Consume all 3 burst tokens at tick 100
        assert!(limiter.check_and_consume(account, 100).is_ok());
        assert!(limiter.check_and_consume(account, 100).is_ok());
        assert!(limiter.check_and_consume(account, 100).is_ok());

        // 4th message should be rejected
        assert!(limiter.check_and_consume(account, 100).is_err());

        // At tick 119 (19 ticks later), still rejected (< 20 ticks)
        assert!(limiter.check_and_consume(account, 119).is_err());

        // At tick 120 (20 ticks later), 1 token replenished
        assert!(limiter.check_and_consume(account, 120).is_ok());
        // Empty again
        assert!(limiter.check_and_consume(account, 120).is_err());
    }
}
