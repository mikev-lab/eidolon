//! Dynamic per-client bandwidth governor and studio density profiles.
//!
//! Enforces byte budgets using token-bucket rate limiting, prioritizing high-relevance
//! combat entities and shedding distant tiers when egress budgets are saturated.

use core::fmt;

/// Studio bandwidth and density profiles configuring per-client wire budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DensityProfile {
    /// Strict budget profile for mobile networks (< 1.2 KB/s = 1,200 bytes/sec).
    #[default]
    BudgetMobile,
    /// Standard MMO profile for balanced desktop and console play (~2.5 KB/s = 2,500 bytes/sec).
    StandardMMO,
    /// High-density fleet battle and siege profile for 2,000+ combatants (~4.5 KB/s = 4,500 bytes/sec).
    MassiveFleetOrSiege,
    /// Custom studio profile with explicit bytes per second limit.
    Custom {
        /// Maximum allowed egress bytes per second.
        max_bytes_per_sec: u32,
    },
}

impl DensityProfile {
    /// Returns the maximum allowed egress bytes per second for this profile.
    #[inline]
    pub const fn max_bytes_per_sec(&self) -> u32 {
        match *self {
            Self::BudgetMobile => 1200,
            Self::StandardMMO => 2500,
            Self::MassiveFleetOrSiege => 4500,
            Self::Custom { max_bytes_per_sec } => max_bytes_per_sec,
        }
    }

    /// Returns the token replenishment in bytes per 20 Hz simulation tick (50ms).
    #[inline]
    pub const fn bytes_per_tick(&self) -> u32 {
        self.max_bytes_per_sec() / 20
    }

    /// Maximum burst bucket capacity in bytes (typically 3 ticks worth of tokens).
    #[inline]
    pub const fn bucket_capacity(&self) -> u32 {
        self.bytes_per_tick() * 3
    }
}

impl fmt::Display for DensityProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BudgetMobile => write!(f, "BudgetMobile (<1.2 KB/s)"),
            Self::StandardMMO => write!(f, "StandardMMO (~2.5 KB/s)"),
            Self::MassiveFleetOrSiege => write!(f, "MassiveFleetOrSiege (~4.5 KB/s)"),
            Self::Custom { max_bytes_per_sec } => write!(f, "Custom ({} B/s)", max_bytes_per_sec),
        }
    }
}

/// Per-client egress token bucket tracking available bandwidth budget.
#[derive(Debug, Clone, Copy)]
pub struct ClientTokenBucket {
    /// Available tokens in bytes.
    tokens: u32,
    /// Total bytes consumed by this client in the current monitoring epoch.
    bytes_sent_epoch: u64,
}

impl ClientTokenBucket {
    /// Constructs a token bucket initialized to full capacity.
    #[inline]
    pub const fn new(initial_tokens: u32) -> Self {
        Self {
            tokens: initial_tokens,
            bytes_sent_epoch: 0,
        }
    }

    /// Replenishes tokens based on profile rate, capping at maximum capacity.
    #[inline]
    pub fn replenish(&mut self, add_tokens: u32, max_capacity: u32) {
        self.tokens = self.tokens.saturating_add(add_tokens).min(max_capacity);
    }

    /// Checks if the requested byte count can be sent within budget.
    #[inline]
    pub const fn can_send(&self, bytes: usize) -> bool {
        self.tokens >= (bytes as u32)
    }

    /// Consumes tokens for a sent payload.
    #[inline]
    pub fn consume(&mut self, bytes: usize) -> bool {
        let b = bytes as u32;
        if self.tokens >= b {
            self.tokens -= b;
            self.bytes_sent_epoch = self.bytes_sent_epoch.saturating_add(b as u64);
            true
        } else {
            false
        }
    }

    /// Current available token count in bytes.
    #[inline]
    pub const fn available_tokens(&self) -> u32 {
        self.tokens
    }

    /// Total bytes sent in the current monitoring epoch.
    #[inline]
    pub const fn bytes_sent_epoch(&self) -> u64 {
        self.bytes_sent_epoch
    }
}

/// Multi-client bandwidth governor regulating egress traffic according to studio density profiles.
///
/// Pre-allocates token buckets for up to `max_clients` to ensure zero runtime heap allocations
/// during hot simulation ticks.
#[derive(Debug)]
pub struct BandwidthGovernor {
    profile: DensityProfile,
    max_clients: usize,
    buckets: Vec<ClientTokenBucket>,
    active: Vec<bool>,
}

impl BandwidthGovernor {
    /// Constructs a bandwidth governor with pre-allocated client slots.
    pub fn new(max_clients: usize, profile: DensityProfile) -> Self {
        let init_tokens = profile.bucket_capacity();
        Self {
            profile,
            max_clients,
            buckets: vec![ClientTokenBucket::new(init_tokens); max_clients],
            active: vec![false; max_clients],
        }
    }

    /// Registers a client slot for bandwidth governance.
    pub fn register_client(&mut self, client_id: u32) {
        let idx = client_id as usize;
        if idx < self.max_clients {
            self.active[idx] = true;
            self.buckets[idx] = ClientTokenBucket::new(self.profile.bucket_capacity());
        }
    }

    /// Unregisters a client slot.
    pub fn unregister_client(&mut self, client_id: u32) {
        let idx = client_id as usize;
        if idx < self.max_clients {
            self.active[idx] = false;
        }
    }

    /// Configures the active density profile.
    pub fn set_profile(&mut self, profile: DensityProfile) {
        self.profile = profile;
    }

    /// Returns the active density profile.
    #[inline]
    pub const fn profile(&self) -> DensityProfile {
        self.profile
    }

    /// Advances the simulation tick, replenishing token buckets for all active clients.
    pub fn tick(&mut self) {
        let add = self.profile.bytes_per_tick();
        let cap = self.profile.bucket_capacity();
        for i in 0..self.max_clients {
            if self.active[i] {
                self.buckets[i].replenish(add, cap);
            }
        }
    }

    /// Evaluates if a client has sufficient bandwidth tokens to send `bytes`.
    #[inline]
    pub fn can_send(&self, client_id: u32, bytes: usize) -> bool {
        let idx = client_id as usize;
        if idx < self.max_clients && self.active[idx] {
            self.buckets[idx].can_send(bytes)
        } else {
            false
        }
    }

    /// Consumes tokens from the client's bucket if sufficient tokens are available.
    pub fn consume(&mut self, client_id: u32, bytes: usize) -> bool {
        let idx = client_id as usize;
        if idx < self.max_clients && self.active[idx] {
            self.buckets[idx].consume(bytes)
        } else {
            false
        }
    }

    /// Evaluates whether an entity update in the given tier should be admitted based on client token headroom.
    ///
    /// Under high congestion, lower tiers are shed first:
    /// - Tier 4 (Macro): requires >= 75% bucket capacity
    /// - Tier 3 (Horizon): requires >= 50% bucket capacity
    /// - Tier 2 (Midfield): requires >= 25% bucket capacity
    /// - Tier 0 and 1 (Immediate and Tactical): permitted as long as tokens >= payload size
    pub fn should_admit_tier(&self, client_id: u32, tier_index: u8, payload_bytes: usize) -> bool {
        let idx = client_id as usize;
        if idx >= self.max_clients || !self.active[idx] {
            return false;
        }

        let bucket = &self.buckets[idx];
        if !bucket.can_send(payload_bytes) {
            return false;
        }

        let cap = self.profile.bucket_capacity();
        let tokens = bucket.available_tokens();

        match tier_index {
            // Immediate (0) and Tactical (1): strictly admit if tokens >= payload
            0 | 1 => true,
            // Midfield (2): requires at least 25% token headroom
            2 => tokens >= (cap / 4),
            // Horizon (3): requires at least 50% token headroom
            3 => tokens >= (cap / 2),
            // Macro (4): requires at least 75% token headroom
            _ => tokens >= (cap * 3 / 4),
        }
    }

    /// Returns the available tokens for a client.
    pub fn available_tokens(&self, client_id: u32) -> u32 {
        let idx = client_id as usize;
        if idx < self.max_clients && self.active[idx] {
            self.buckets[idx].available_tokens()
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_density_profile_rates() {
        let mobile = DensityProfile::BudgetMobile;
        assert_eq!(mobile.max_bytes_per_sec(), 1200);
        assert_eq!(mobile.bytes_per_tick(), 60);
        assert_eq!(mobile.bucket_capacity(), 180);

        let mmo = DensityProfile::StandardMMO;
        assert_eq!(mmo.max_bytes_per_sec(), 2500);
        assert_eq!(mmo.bytes_per_tick(), 125);
        assert_eq!(mmo.bucket_capacity(), 375);

        let siege = DensityProfile::MassiveFleetOrSiege;
        assert_eq!(siege.max_bytes_per_sec(), 4500);
        assert_eq!(siege.bytes_per_tick(), 225);
        assert_eq!(siege.bucket_capacity(), 675);
    }

    #[test]
    fn test_token_bucket_replenish_and_consume() {
        let mut bucket = ClientTokenBucket::new(100);
        assert!(bucket.can_send(50));
        assert!(bucket.consume(50));
        assert_eq!(bucket.available_tokens(), 50);

        // Cannot consume more than remaining tokens
        assert!(!bucket.can_send(60));
        assert!(!bucket.consume(60));
        assert_eq!(bucket.available_tokens(), 50);

        // Replenish with cap
        bucket.replenish(40, 100);
        assert_eq!(bucket.available_tokens(), 90);

        // Overflow cap
        bucket.replenish(50, 100);
        assert_eq!(bucket.available_tokens(), 100);
    }

    #[test]
    fn test_bandwidth_governor_tier_shedding_under_pressure() {
        let mut gov = BandwidthGovernor::new(10, DensityProfile::BudgetMobile);
        gov.register_client(1);

        let cap = DensityProfile::BudgetMobile.bucket_capacity(); // 180
        assert_eq!(gov.available_tokens(1), cap);

        // At full capacity (180 tokens): all tiers admitted
        assert!(gov.should_admit_tier(1, 0, 7));
        assert!(gov.should_admit_tier(1, 1, 7));
        assert!(gov.should_admit_tier(1, 2, 5));
        assert!(gov.should_admit_tier(1, 3, 4));
        assert!(gov.should_admit_tier(1, 4, 2));

        // Consume down to 100 tokens (< 75% of 180 = 135): Macro tier shed
        assert!(gov.consume(1, 80));
        assert_eq!(gov.available_tokens(1), 100);
        assert!(!gov.should_admit_tier(1, 4, 2)); // Macro shed
        assert!(gov.should_admit_tier(1, 3, 4)); // Horizon still admitted (>= 90)

        // Consume down to 60 tokens (< 50% of 180 = 90): Horizon tier shed
        assert!(gov.consume(1, 40));
        assert_eq!(gov.available_tokens(1), 60);
        assert!(!gov.should_admit_tier(1, 3, 4)); // Horizon shed
        assert!(gov.should_admit_tier(1, 2, 5)); // Midfield still admitted (>= 45)

        // Consume down to 30 tokens (< 25% of 180 = 45): Midfield tier shed
        assert!(gov.consume(1, 30));
        assert_eq!(gov.available_tokens(1), 30);
        assert!(!gov.should_admit_tier(1, 2, 5)); // Midfield shed
        assert!(gov.should_admit_tier(1, 0, 7)); // Immediate strictly preserved!
        assert!(gov.should_admit_tier(1, 1, 7)); // Tactical strictly preserved!

        // Tick advances: tokens replenish (+60)
        gov.tick();
        assert_eq!(gov.available_tokens(1), 90);
        assert!(gov.should_admit_tier(1, 3, 4)); // Horizon admitted again
    }

    #[test]
    fn test_bandwidth_governor_multi_client_isolation() {
        let mut gov = BandwidthGovernor::new(10, DensityProfile::StandardMMO);
        gov.register_client(1);
        gov.register_client(2);

        assert!(gov.consume(1, 300));
        assert_eq!(gov.available_tokens(1), 75);
        assert_eq!(gov.available_tokens(2), 375); // Client 2 unaffected
    }
}
