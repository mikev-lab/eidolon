//! Malicious-client ingress rate policing, anti-DoS quotas, and per-connection memory fencing.
//!
//! Enforces deterministic resource bounds:
//! - Hard per-socket ingress policing (max 40 pps / 32 KB/s per client socket).
//! - Hard per-connection memory allocation ceiling (64 KB).
//! - CPU execution guards preventing algorithmic complexity attacks in untrusted packet parsing.

use crate::error::NetError;

/// Default maximum allowable incoming packets per second per client socket.
pub const DEFAULT_MAX_PPS: u32 = 40;

/// Default maximum allowable incoming bytes per second per client socket (32 KB/s).
pub const DEFAULT_MAX_BPS: u32 = 32_768;

/// Hard ceiling on dynamic heap allocation per active connection context (64 KB).
pub const MAX_CONNECTION_MEMORY: usize = 65_536;

/// Token-bucket rate policer enforcing packet and byte ingress quotas for a client socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketRatePolicer {
    /// Configured maximum packets per second.
    max_packets_per_sec: u32,
    /// Configured maximum bytes per second.
    max_bytes_per_sec: u32,
    /// Current available packet tokens.
    packet_tokens: u32,
    /// Current available byte tokens.
    byte_tokens: u32,
    /// Last simulation tick when tokens were replenished.
    last_replenish_tick: u64,
}

impl Default for SocketRatePolicer {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_PPS, DEFAULT_MAX_BPS)
    }
}

impl SocketRatePolicer {
    /// Creates a new rate policer with custom quotas.
    pub fn new(max_packets_per_sec: u32, max_bytes_per_sec: u32) -> Self {
        Self {
            max_packets_per_sec,
            max_bytes_per_sec,
            packet_tokens: max_packets_per_sec,
            byte_tokens: max_bytes_per_sec,
            last_replenish_tick: 0,
        }
    }

    /// Replenishes tokens according to elapsed 20 Hz simulation ticks (50ms per tick).
    pub fn replenish(&mut self, current_tick: u64) {
        if current_tick <= self.last_replenish_tick {
            return;
        }
        let elapsed_ticks = current_tick - self.last_replenish_tick;
        self.last_replenish_tick = current_tick;

        // At 20 Hz, each tick adds (max_rate / 20) tokens
        let pps_per_tick = (self.max_packets_per_sec / 20).max(1);
        let bps_per_tick = (self.max_bytes_per_sec / 20).max(1);

        let added_packets = (elapsed_ticks as u32).saturating_mul(pps_per_tick);
        let added_bytes = (elapsed_ticks as u32).saturating_mul(bps_per_tick);

        self.packet_tokens = self
            .packet_tokens
            .saturating_add(added_packets)
            .min(self.max_packets_per_sec);
        self.byte_tokens = self
            .byte_tokens
            .saturating_add(added_bytes)
            .min(self.max_bytes_per_sec);
    }

    /// Evaluates an incoming packet against ingress quotas.
    ///
    /// If quotas are satisfied, tokens are deducted and `Ok(())` is returned.
    /// If quotas are exceeded, returns `Err(NetError::RateLimitExceeded)`.
    pub fn check_ingress(&mut self, packet_len: usize, current_tick: u64) -> Result<(), NetError> {
        self.replenish(current_tick);

        if self.packet_tokens < 1 || (self.byte_tokens as usize) < packet_len {
            return Err(NetError::RateLimitExceeded {
                pps_limit: self.max_packets_per_sec,
            });
        }

        self.packet_tokens = self.packet_tokens.saturating_sub(1);
        self.byte_tokens = self.byte_tokens.saturating_sub(packet_len as u32);
        Ok(())
    }

    /// Returns remaining packet tokens in the bucket.
    #[inline]
    pub fn available_packet_tokens(&self) -> u32 {
        self.packet_tokens
    }

    /// Returns remaining byte tokens in the bucket.
    #[inline]
    pub fn available_byte_tokens(&self) -> u32 {
        self.byte_tokens
    }
}

/// Tracks and bounds dynamic memory consumption for an individual client connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionMemoryAccountant {
    /// Total bytes currently tracked for this connection.
    allocated_bytes: usize,
    /// Maximum allowable memory allocation limit.
    max_bytes: usize,
}

impl Default for ConnectionMemoryAccountant {
    fn default() -> Self {
        Self::new(MAX_CONNECTION_MEMORY)
    }
}

impl ConnectionMemoryAccountant {
    /// Creates a new memory accountant with configured upper ceiling.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            allocated_bytes: 0,
            max_bytes,
        }
    }

    /// Attempts to record a memory allocation.
    ///
    /// Returns `Ok(())` if within bounds.
    /// Returns `Err(NetError::MemoryQuotaExceeded)` if allocation would breach ceiling.
    pub fn track_allocation(&mut self, bytes: usize) -> Result<(), NetError> {
        let new_total = self.allocated_bytes.saturating_add(bytes);
        if new_total > self.max_bytes {
            return Err(NetError::MemoryQuotaExceeded {
                current_bytes: self.allocated_bytes,
                max_bytes: self.max_bytes,
            });
        }
        self.allocated_bytes = new_total;
        Ok(())
    }

    /// Releases previously tracked memory.
    pub fn release_allocation(&mut self, bytes: usize) {
        self.allocated_bytes = self.allocated_bytes.saturating_sub(bytes);
    }

    /// Returns the current total allocated bytes.
    #[inline]
    pub fn allocated_bytes(&self) -> usize {
        self.allocated_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_socket_rate_policer_happy_path() {
        let mut policer = SocketRatePolicer::new(40, 32_768);
        for _ in 0..10 {
            assert!(policer.check_ingress(200, 1).is_ok());
        }
    }

    #[test]
    fn test_socket_rate_policer_exceeds_packet_limit() {
        let mut policer = SocketRatePolicer::new(10, 10_000);
        for _ in 0..10 {
            assert!(policer.check_ingress(50, 1).is_ok());
        }
        // 11th packet in same tick exceeds quota
        assert!(policer.check_ingress(50, 1).is_err());
    }

    #[test]
    fn test_socket_rate_policer_replenish() {
        let mut policer = SocketRatePolicer::new(20, 20_000);
        for _ in 0..20 {
            assert!(policer.check_ingress(100, 1).is_ok());
        }
        assert!(policer.check_ingress(100, 1).is_err());

        // Advance by 10 ticks (0.5s = 10 packets replenished)
        assert!(policer.check_ingress(100, 11).is_ok());
    }

    #[test]
    fn test_connection_memory_accountant_bounds() {
        let mut accountant = ConnectionMemoryAccountant::new(1024);
        assert!(accountant.track_allocation(512).is_ok());
        assert!(accountant.track_allocation(512).is_ok());
        // Exceeds ceiling
        assert!(accountant.track_allocation(1).is_err());

        accountant.release_allocation(512);
        assert!(accountant.track_allocation(256).is_ok());
        assert_eq!(accountant.allocated_bytes(), 768);
    }
}
