//! High-resolution 20 Hz fixed-step simulation tick coordinator and watchdog.
//!
//! Enforces exact 50ms simulation steps using monotonic clocks with accumulated drift correction,
//! adaptive spatial load shedding under CPU spikes, and watchdog circuit breakers.

use std::time::{Duration, Instant};

use eidolon_spatial::aoi::LoadSheddingLevel;

use crate::clock::{ClockGovernor, ClockGovernorConfig};

/// Standard simulation tick interval (50,000 microseconds = 50ms at 20 Hz).
pub const DEFAULT_TICK_MICROS: u64 = 50_000;

/// Tick execution time threshold (40ms / 80% budget) triggering Level 1 load shedding.
pub const LEVEL1_SHEDDING_MICROS: u64 = 40_000;

/// Tick execution time threshold (45ms / 90% budget) triggering Level 2 load shedding.
pub const LEVEL2_SHEDDING_MICROS: u64 = 45_000;

/// Watchdog circuit breaker threshold (49ms / 98% budget) aborting remaining non-critical work.
pub const WATCHDOG_CIRCUIT_BREAKER_MICROS: u64 = 49_000;

/// Monotonic metrics recorded across simulation execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TickMetrics {
    /// Total number of authoritative ticks simulated.
    pub total_ticks: u64,
    /// Total number of ticks that exceeded the 50ms budget.
    pub overrun_ticks: u64,
    /// Maximum observed tick duration in microseconds.
    pub max_duration_micros: u64,
    /// Current spatial load shedding level.
    pub shedding_level: LoadSheddingLevel,
    /// Number of times the watchdog circuit breaker tripped.
    pub watchdog_trips: u64,
}

/// Fixed-interval tick coordinator orchestrating the 20 Hz authoritative game loop.
#[derive(Debug)]
pub struct TickCoordinator {
    tick_interval: Duration,
    current_tick: u64,
    last_tick_instant: Instant,
    shedding_level: LoadSheddingLevel,
    consecutive_normal_ticks: u32,
    metrics: TickMetrics,
    clock_governor: ClockGovernor,
}

impl TickCoordinator {
    /// Constructs a new `TickCoordinator` with the specified tick rate in Hertz.
    pub fn new(tick_rate_hz: u32) -> Self {
        let micros = 1_000_000 / (tick_rate_hz.max(1) as u64);
        let now = Instant::now();
        let tick_interval = Duration::from_micros(micros);
        let clock_governor = ClockGovernor::with_config(ClockGovernorConfig {
            tick_interval,
            ..ClockGovernorConfig::default()
        });

        Self {
            tick_interval,
            current_tick: 0,
            last_tick_instant: now,
            shedding_level: LoadSheddingLevel::None,
            consecutive_normal_ticks: 0,
            metrics: TickMetrics::default(),
            clock_governor,
        }
    }

    /// Returns the current simulation tick index.
    #[inline]
    pub fn current_tick(&self) -> u64 {
        self.current_tick
    }

    /// Returns the active spatial load shedding level.
    #[inline]
    pub fn shedding_level(&self) -> LoadSheddingLevel {
        self.shedding_level
    }

    /// Returns a copy of the accumulated simulation metrics.
    #[inline]
    pub fn metrics(&self) -> TickMetrics {
        self.metrics
    }

    /// Records the execution duration of a completed tick and evaluates load shedding.
    ///
    /// Returns the updated `LoadSheddingLevel` to apply during the subsequent tick.
    pub fn record_tick_execution(&mut self, duration: Duration) -> LoadSheddingLevel {
        self.current_tick += 1;
        self.metrics.total_ticks += 1;

        let duration_micros = duration.as_micros() as u64;
        if duration_micros > self.metrics.max_duration_micros {
            self.metrics.max_duration_micros = duration_micros;
        }

        // Watchdog circuit breaker check
        if duration_micros >= WATCHDOG_CIRCUIT_BREAKER_MICROS {
            self.metrics.watchdog_trips += 1;
        }

        // Evaluate tick overrun
        if duration >= self.tick_interval {
            self.metrics.overrun_ticks += 1;
            self.consecutive_normal_ticks = 0;
        }

        // Adaptive load shedding state machine with recovery hysteresis
        if duration_micros >= LEVEL2_SHEDDING_MICROS {
            self.shedding_level = LoadSheddingLevel::Level2;
            self.consecutive_normal_ticks = 0;
        } else if duration_micros >= LEVEL1_SHEDDING_MICROS {
            if self.shedding_level < LoadSheddingLevel::Level1 {
                self.shedding_level = LoadSheddingLevel::Level1;
            }
            self.consecutive_normal_ticks = 0;
        } else {
            self.consecutive_normal_ticks += 1;

            // Recovery hysteresis
            if self.shedding_level == LoadSheddingLevel::Level2
                && self.consecutive_normal_ticks >= 5
            {
                self.shedding_level = LoadSheddingLevel::Level1;
                self.consecutive_normal_ticks = 0;
            } else if self.shedding_level == LoadSheddingLevel::Level1
                && self.consecutive_normal_ticks >= 10
            {
                self.shedding_level = LoadSheddingLevel::None;
                self.consecutive_normal_ticks = 0;
            }
        }

        self.metrics.shedding_level = self.shedding_level;
        self.shedding_level
    }

    /// Returns an immutable reference to the clock governor.
    #[inline]
    pub const fn clock_governor(&self) -> &ClockGovernor {
        &self.clock_governor
    }

    /// Returns a mutable reference to the clock governor.
    #[inline]
    pub fn clock_governor_mut(&mut self) -> &mut ClockGovernor {
        &mut self.clock_governor
    }

    /// Sleeps or yields remaining tick headroom to maintain exact 20 Hz pacing.
    ///
    /// Dynamically aligns to monotonic tick target boundaries to eliminate OS scheduler drift,
    /// absorbing virtualization suspend/resume pauses and clamping catch-up cascades.
    pub fn sleep_headroom(&mut self, execution_duration: Duration) {
        self.clock_governor.sleep_pacing(execution_duration);
        self.last_tick_instant = Instant::now();
    }
}

impl Default for TickCoordinator {
    fn default() -> Self {
        Self::new(20)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tick_coordinator_initial_state() {
        let coordinator = TickCoordinator::new(20);
        assert_eq!(coordinator.current_tick(), 0);
        assert_eq!(coordinator.shedding_level(), LoadSheddingLevel::None);
        assert_eq!(coordinator.metrics().total_ticks, 0);
        assert_eq!(coordinator.metrics().overrun_ticks, 0);
    }

    #[test]
    fn test_adaptive_load_shedding_escalation_and_hysteresis() {
        let mut coordinator = TickCoordinator::new(20);

        // Normal ticks (<40ms)
        let level = coordinator.record_tick_execution(Duration::from_millis(15));
        assert_eq!(level, LoadSheddingLevel::None);

        // Spike to 42ms -> Level 1
        let level = coordinator.record_tick_execution(Duration::from_millis(42));
        assert_eq!(level, LoadSheddingLevel::Level1);

        // Spike to 47ms -> Level 2
        let level = coordinator.record_tick_execution(Duration::from_millis(47));
        assert_eq!(level, LoadSheddingLevel::Level2);

        // Circuit breaker trip at 50ms
        let level = coordinator.record_tick_execution(Duration::from_millis(50));
        assert_eq!(level, LoadSheddingLevel::Level2);
        assert_eq!(coordinator.metrics().watchdog_trips, 1);
        assert_eq!(coordinator.metrics().overrun_ticks, 1);

        // Fast ticks: verify recovery hysteresis from Level 2 to Level 1 (5 ticks)
        for _ in 0..4 {
            assert_eq!(
                coordinator.record_tick_execution(Duration::from_millis(20)),
                LoadSheddingLevel::Level2
            );
        }
        // 5th fast tick recovers to Level 1
        assert_eq!(
            coordinator.record_tick_execution(Duration::from_millis(20)),
            LoadSheddingLevel::Level1
        );

        // Fast ticks: verify recovery hysteresis from Level 1 to None (10 ticks)
        for _ in 0..9 {
            assert_eq!(
                coordinator.record_tick_execution(Duration::from_millis(20)),
                LoadSheddingLevel::Level1
            );
        }
        // 10th fast tick recovers to None
        assert_eq!(
            coordinator.record_tick_execution(Duration::from_millis(20)),
            LoadSheddingLevel::None
        );
    }
}
