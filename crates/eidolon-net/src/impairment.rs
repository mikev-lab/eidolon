//! Synthetic real-world network impairment harness.
//!
//! Simulates adversarial real-world network conditions including continuous/burst packet loss,
//! asymmetric latency, jitter (up to 150ms), packet reordering, and packet duplication
//! with zero external third-party dependencies and zero dynamic heap allocations.

use crate::protocol::MAX_PACKET_SIZE;

/// Deterministic fast pseudo-random number generator (XorShift64).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FastPrng {
    state: u64,
}

impl FastPrng {
    /// Creates a new PRNG seeded with a non-zero initial state.
    #[inline]
    pub const fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x5EED_BEEF_CAFE_BABE
            } else {
                seed
            },
        }
    }

    /// Generates the next pseudo-random 64-bit unsigned integer.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Generates an integer in the inclusive range `[min, max]`.
    #[inline]
    pub fn gen_range(&mut self, min: u64, max: u64) -> u64 {
        if min >= max {
            min
        } else {
            min + (self.next_u64() % (max - min + 1))
        }
    }

    /// Returns true if a random probability check succeeds against basis points (1/100th of 1%).
    ///
    /// e.g. 100 bps = 1.0%, 500 bps = 5.0%, 10,000 bps = 100.0%.
    #[inline]
    pub fn check_bps(&mut self, basis_points: u32) -> bool {
        if basis_points == 0 {
            false
        } else if basis_points >= 10_000 {
            true
        } else {
            (self.next_u64() % 10_000) < (basis_points as u64)
        }
    }
}

/// Configuration parameters for synthetic network impairment simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImpairmentConfig {
    /// Packet loss rate in basis points (100 = 1.0%, 500 = 5.0%).
    pub loss_bps: u32,
    /// Number of consecutive packets dropped when burst loss triggers.
    pub burst_loss_length: u32,
    /// Minimum one-way simulated latency in microseconds.
    pub min_latency_micros: u64,
    /// Maximum one-way simulated latency in microseconds.
    pub max_latency_micros: u64,
    /// Asymmetric jitter added or subtracted in microseconds.
    pub jitter_micros: u64,
    /// Reordering probability in basis points.
    pub reorder_bps: u32,
    /// Duplication probability in basis points.
    pub duplication_bps: u32,
}

impl Default for ImpairmentConfig {
    fn default() -> Self {
        Self {
            loss_bps: 0,
            burst_loss_length: 1,
            min_latency_micros: 0,
            max_latency_micros: 0,
            jitter_micros: 0,
            reorder_bps: 0,
            duplication_bps: 0,
        }
    }
}

impl ImpairmentConfig {
    /// Preset: Healthy fiber connection (<5ms latency, zero loss).
    pub const fn fiber_clean() -> Self {
        Self {
            loss_bps: 0,
            burst_loss_length: 1,
            min_latency_micros: 1_000,
            max_latency_micros: 5_000,
            jitter_micros: 500,
            reorder_bps: 0,
            duplication_bps: 0,
        }
    }

    /// Preset: Standard broadband connection (20-40ms latency, 1.0% packet loss).
    pub const fn standard_broadband() -> Self {
        Self {
            loss_bps: 100, // 1.0%
            burst_loss_length: 1,
            min_latency_micros: 20_000,
            max_latency_micros: 40_000,
            jitter_micros: 5_000,
            reorder_bps: 10, // 0.1%
            duplication_bps: 5,
        }
    }

    /// Preset: Adversarial mobile LTE network with 5.0% burst loss and 150ms jitter.
    pub const fn adversarial_mobile() -> Self {
        Self {
            loss_bps: 500, // 5.0%
            burst_loss_length: 3,
            min_latency_micros: 50_000,
            max_latency_micros: 150_000,
            jitter_micros: 150_000,
            reorder_bps: 200,     // 2.0%
            duplication_bps: 100, // 1.0%
        }
    }
}

/// A buffered packet awaiting delivery at a future simulated timestamp.
#[derive(Debug, Clone, Copy)]
pub struct DelayedPacket {
    /// Payload buffer.
    pub data: [u8; MAX_PACKET_SIZE],
    /// Valid payload length in bytes.
    pub len: usize,
    /// Scheduled delivery timestamp in microseconds.
    pub deliver_at_micros: u64,
}

impl Default for DelayedPacket {
    fn default() -> Self {
        Self {
            data: [0u8; MAX_PACKET_SIZE],
            len: 0,
            deliver_at_micros: 0,
        }
    }
}

/// Action outcome determined by the network impairment simulator for an egress packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImpairmentOutcome {
    /// Packet dropped due to simulated packet loss.
    Dropped,
    /// Packet passed immediately without delay.
    PassedImmediate,
    /// Packet buffered for delayed arrival due to simulated latency or jitter.
    DelayedUntil(u64),
    /// Packet duplicated; original sent immediately and clone queued for delivery.
    Duplicated,
}

/// Cumulative telemetry metrics recorded by the impairment harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImpairmentMetrics {
    /// Total packets submitted to the harness.
    pub total_processed: u64,
    /// Total packets dropped by loss or burst loss.
    pub dropped_packets: u64,
    /// Total packets subjected to delay queues.
    pub delayed_packets: u64,
    /// Total packets duplicated.
    pub duplicated_packets: u64,
    /// Total packets passed through immediately.
    pub passed_immediate: u64,
}

/// Real-world synthetic network impairment coordinator.
#[derive(Debug)]
pub struct NetworkImpairmentHarness<const CAPACITY: usize> {
    config: ImpairmentConfig,
    prng: FastPrng,
    delayed_slots: [Option<DelayedPacket>; CAPACITY],
    delayed_count: usize,
    burst_loss_remaining: u32,
    metrics: ImpairmentMetrics,
}

impl<const CAPACITY: usize> NetworkImpairmentHarness<CAPACITY> {
    /// Creates a new network impairment harness with the given configuration and PRNG seed.
    pub fn new(config: ImpairmentConfig, seed: u64) -> Self {
        Self {
            config,
            prng: FastPrng::new(seed),
            delayed_slots: [None; CAPACITY],
            delayed_count: 0,
            burst_loss_remaining: 0,
            metrics: ImpairmentMetrics::default(),
        }
    }

    /// Returns telemetry metrics recorded by the harness.
    #[inline]
    pub const fn metrics(&self) -> &ImpairmentMetrics {
        &self.metrics
    }

    /// Returns the number of packets currently held in the delay queue.
    #[inline]
    pub const fn delayed_count(&self) -> usize {
        self.delayed_count
    }

    /// Processes an outgoing packet through the impairment pipeline.
    ///
    /// Evaluates packet loss, latency, jitter, reordering, and duplication.
    /// If the packet is passed immediately, its bytes are copied into `out_immediate` and `Some(len)` is returned.
    pub fn process_egress(
        &mut self,
        payload: &[u8],
        now_micros: u64,
        out_immediate: &mut [u8],
    ) -> ImpairmentOutcome {
        self.metrics.total_processed += 1;

        // 1. Evaluate Packet Loss (Burst or Stochastic)
        if self.burst_loss_remaining > 0 {
            self.burst_loss_remaining -= 1;
            self.metrics.dropped_packets += 1;
            return ImpairmentOutcome::Dropped;
        }

        if self.prng.check_bps(self.config.loss_bps) {
            self.burst_loss_remaining = self.config.burst_loss_length.saturating_sub(1);
            self.metrics.dropped_packets += 1;
            return ImpairmentOutcome::Dropped;
        }

        // 2. Compute Latency + Jitter
        let base_delay = if self.config.max_latency_micros > self.config.min_latency_micros {
            self.prng.gen_range(
                self.config.min_latency_micros,
                self.config.max_latency_micros,
            )
        } else {
            self.config.min_latency_micros
        };

        let jitter = if self.config.jitter_micros > 0 {
            self.prng.gen_range(0, self.config.jitter_micros)
        } else {
            0
        };

        let total_delay_micros = base_delay.saturating_add(jitter);

        // 3. Evaluate Duplication
        let duplicate = self.prng.check_bps(self.config.duplication_bps);

        // 4. Evaluate Reordering / Delay
        let should_delay = total_delay_micros > 0 || self.prng.check_bps(self.config.reorder_bps);

        if should_delay {
            let deliver_at = now_micros.saturating_add(total_delay_micros);
            let queued = self.enqueue_delayed(payload, deliver_at);

            if queued {
                self.metrics.delayed_packets += 1;

                if duplicate {
                    self.metrics.duplicated_packets += 1;
                    // Copy one immediate instance
                    let copy_len = payload.len().min(out_immediate.len());
                    out_immediate[..copy_len].copy_from_slice(&payload[..copy_len]);
                    return ImpairmentOutcome::Duplicated;
                }

                return ImpairmentOutcome::DelayedUntil(deliver_at);
            }
        }

        // 5. Immediate Transmission
        let copy_len = payload.len().min(out_immediate.len());
        out_immediate[..copy_len].copy_from_slice(&payload[..copy_len]);
        self.metrics.passed_immediate += 1;

        if duplicate {
            self.metrics.duplicated_packets += 1;
            // Also queue a duplicate for delayed arrival (15ms later)
            let _ = self.enqueue_delayed(payload, now_micros.saturating_add(15_000));
            ImpairmentOutcome::Duplicated
        } else {
            ImpairmentOutcome::PassedImmediate
        }
    }

    /// Enqueues a packet into the fixed-capacity delay buffer.
    fn enqueue_delayed(&mut self, payload: &[u8], deliver_at_micros: u64) -> bool {
        if self.delayed_count >= CAPACITY {
            // Buffer full, drop under delay backpressure
            return false;
        }

        for slot in self.delayed_slots.iter_mut() {
            if slot.is_none() {
                let mut pkt = DelayedPacket {
                    data: [0u8; MAX_PACKET_SIZE],
                    len: payload.len().min(MAX_PACKET_SIZE),
                    deliver_at_micros,
                };
                pkt.data[..pkt.len].copy_from_slice(&payload[..pkt.len]);
                *slot = Some(pkt);
                self.delayed_count += 1;
                return true;
            }
        }
        false
    }

    /// Drains the next matured packet ready for transmission into `dest`.
    ///
    /// Returns `Some(bytes_written)` if a matured packet was delivered, or `None` if no packets are ready.
    pub fn drain_ready(&mut self, now_micros: u64, dest: &mut [u8]) -> Option<usize> {
        let mut best_idx: Option<usize> = None;
        let mut oldest_timestamp = u64::MAX;

        // Find the earliest matured packet
        for (i, slot) in self.delayed_slots.iter().enumerate() {
            if let Some(ref pkt) = slot {
                if pkt.deliver_at_micros <= now_micros && pkt.deliver_at_micros < oldest_timestamp {
                    oldest_timestamp = pkt.deliver_at_micros;
                    best_idx = Some(i);
                }
            }
        }

        if let Some(idx) = best_idx {
            if let Some(pkt) = self.delayed_slots[idx].take() {
                self.delayed_count = self.delayed_count.saturating_sub(1);
                let copy_len = pkt.len.min(dest.len());
                dest[..copy_len].copy_from_slice(&pkt.data[..copy_len]);
                return Some(copy_len);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_prng_determinism_and_range() {
        let mut rng1 = FastPrng::new(12345);
        let mut rng2 = FastPrng::new(12345);

        for _ in 0..100 {
            assert_eq!(rng1.next_u64(), rng2.next_u64());
            let val1 = rng1.gen_range(10, 50);
            let val2 = rng2.gen_range(10, 50);
            assert_eq!(val1, val2);
            assert!((10..=50).contains(&val1));
        }
    }

    #[test]
    fn test_impairment_loss_and_burst_drops() {
        // 100% loss test (10,000 bps)
        let config = ImpairmentConfig {
            loss_bps: 10_000,
            burst_loss_length: 2,
            ..Default::default()
        };
        let mut harness: NetworkImpairmentHarness<16> = NetworkImpairmentHarness::new(config, 42);

        let mut out = [0u8; 128];
        let outcome = harness.process_egress(b"test_payload", 1_000, &mut out);
        assert_eq!(outcome, ImpairmentOutcome::Dropped);
        assert_eq!(harness.metrics().dropped_packets, 1);
    }

    #[test]
    fn test_impairment_delay_and_drain_lifecycle() {
        let config = ImpairmentConfig {
            min_latency_micros: 20_000, // 20ms
            max_latency_micros: 20_000,
            jitter_micros: 0,
            ..Default::default()
        };
        let mut harness: NetworkImpairmentHarness<16> = NetworkImpairmentHarness::new(config, 100);

        let mut out = [0u8; 128];
        let outcome = harness.process_egress(b"delayed_data", 10_000, &mut out);

        assert_eq!(outcome, ImpairmentOutcome::DelayedUntil(30_000));
        assert_eq!(harness.delayed_count(), 1);

        // At t = 20,000 µs (not yet ready)
        let mut drain_buf = [0u8; 128];
        assert_eq!(harness.drain_ready(20_000, &mut drain_buf), None);

        // At t = 30,000 µs (now ready)
        let bytes = harness.drain_ready(30_000, &mut drain_buf).unwrap();
        assert_eq!(bytes, 12);
        assert_eq!(&drain_buf[..bytes], b"delayed_data");
        assert_eq!(harness.delayed_count(), 0);
    }
}
