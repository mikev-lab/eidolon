//! Monotonic simulation clock discipline, time warp defense, and hypervisor pause recovery.
//!
//! Decouples the 20 Hz simulation tick loop from wall-clock UTC (SystemTime), providing
//! immunity against NTP step adjustments, and preventing spiral-of-death catch-up cascades
//! during hypervisor virtualization pauses and VM live migrations.

use std::thread;
use std::time::{Duration, Instant};

/// Default suspend threshold (200ms = 4 tick intervals at 20 Hz) indicating a virtualization pause.
pub const DEFAULT_SUSPEND_THRESHOLD_MICROS: u64 = 200_000;

/// Default maximum catch-up ticks allowed in a burst before clamping baseline.
pub const DEFAULT_MAX_CATCH_UP_TICKS: u32 = 4;

/// Result of evaluating tick pacing relative to monotonic target intervals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickPacingAction {
    /// Engine is running on schedule; sleep remaining headroom.
    SleepHeadroom(Duration),
    /// Engine is behind schedule; catch up immediately without sleeping.
    CatchUpImmediate {
        /// Accumulated lag behind target schedule.
        lag: Duration,
        /// Whether lag was clamped to prevent runaway cascades.
        clamped: bool,
    },
    /// A hypervisor suspend/resume or VM migration pause was detected and absorbed.
    VirtualizationResync {
        /// Observed gap duration during hypervisor pause.
        pause_duration: Duration,
        /// Maximum ticks clamped during resync.
        clamped_ticks: u32,
    },
}

/// Metrics recording clock discipline performance, pauses, and drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClockMetrics {
    /// Total ticks processed.
    pub total_ticks: u64,
    /// Number of virtualization suspend/resume pauses detected and absorbed.
    pub virtualization_pauses_absorbed: u64,
    /// Number of times catch-up stepping was clamped to prevent spiral-of-death cascades.
    pub catch_up_clamped_events: u64,
    /// Maximum observed single tick gap duration in microseconds.
    pub max_gap_micros: u64,
    /// Total accumulated lag in microseconds.
    pub total_lag_micros: u64,
}

/// Configuration parameters for simulation clock discipline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockGovernorConfig {
    /// Simulation tick interval (50,000 µs = 50ms for 20 Hz).
    pub tick_interval: Duration,
    /// Duration gap threshold triggering virtualization suspend/resume detection.
    pub suspend_threshold: Duration,
    /// Maximum number of consecutive catch-up ticks allowed before clamping baseline.
    pub max_catch_up_ticks: u32,
}

impl Default for ClockGovernorConfig {
    fn default() -> Self {
        Self {
            tick_interval: Duration::from_micros(50_000),
            suspend_threshold: Duration::from_micros(DEFAULT_SUSPEND_THRESHOLD_MICROS),
            max_catch_up_ticks: DEFAULT_MAX_CATCH_UP_TICKS,
        }
    }
}

/// Authoritative simulation clock governor enforcing hardware monotonic discipline.
#[derive(Debug)]
pub struct ClockGovernor {
    config: ClockGovernorConfig,
    current_tick: u64,
    baseline_tick: u64,
    baseline_instant: Instant,
    last_tick_instant: Instant,
    accumulated_lag: Duration,
    metrics: ClockMetrics,
}

impl ClockGovernor {
    /// Creates a new clock governor with default 20 Hz pacing.
    pub fn new() -> Self {
        Self::with_config(ClockGovernorConfig::default())
    }

    /// Creates a new clock governor with custom configuration.
    pub fn with_config(config: ClockGovernorConfig) -> Self {
        Self::with_baseline(config, Instant::now())
    }

    /// Creates a new clock governor with custom configuration and explicit baseline instant.
    pub fn with_baseline(config: ClockGovernorConfig, baseline: Instant) -> Self {
        Self {
            config,
            current_tick: 0,
            baseline_tick: 0,
            baseline_instant: baseline,
            last_tick_instant: baseline,
            accumulated_lag: Duration::ZERO,
            metrics: ClockMetrics::default(),
        }
    }

    /// Returns the baseline instant used for monotonic schedule pacing.
    #[inline]
    pub const fn baseline_instant(&self) -> Instant {
        self.baseline_instant
    }

    /// Returns the current authoritative simulation tick index.
    #[inline]
    pub const fn current_tick(&self) -> u64 {
        self.current_tick
    }

    /// Returns the configured simulation tick interval.
    #[inline]
    pub const fn tick_interval(&self) -> Duration {
        self.config.tick_interval
    }

    /// Returns a copy of the clock discipline metrics.
    #[inline]
    pub const fn metrics(&self) -> ClockMetrics {
        self.metrics
    }

    /// Evaluates tick pacing given the current instant and execution duration,
    /// detecting virtualization pauses and clamping runaway catch-up cascades.
    pub fn evaluate_pacing(
        &mut self,
        now: Instant,
        _execution_duration: Duration,
    ) -> TickPacingAction {
        self.current_tick += 1;
        self.metrics.total_ticks += 1;

        // Measure actual elapsed time since last tick
        let gap = now.saturating_duration_since(self.last_tick_instant);
        let gap_micros = gap.as_micros() as u64;
        if gap_micros > self.metrics.max_gap_micros {
            self.metrics.max_gap_micros = gap_micros;
        }

        // Check for hypervisor suspend/resume or VM migration pause
        if gap >= self.config.suspend_threshold {
            self.metrics.virtualization_pauses_absorbed += 1;
            // Clamp baseline: preserve at most max_catch_up_ticks of catch-up work
            let clamped_lag = self
                .config
                .tick_interval
                .saturating_mul(self.config.max_catch_up_ticks);
            self.baseline_instant = now.checked_sub(clamped_lag).unwrap_or(now);
            self.baseline_tick = self
                .current_tick
                .saturating_sub(self.config.max_catch_up_ticks as u64);
            self.accumulated_lag = clamped_lag;
            self.last_tick_instant = now;

            return TickPacingAction::VirtualizationResync {
                pause_duration: gap,
                clamped_ticks: self.config.max_catch_up_ticks,
            };
        }

        // Standard monotonic target schedule evaluation relative to active baseline tick
        let elapsed_ticks = self.current_tick.saturating_sub(self.baseline_tick);
        let target_instant = self.baseline_instant
            + self
                .config
                .tick_interval
                .saturating_mul(elapsed_ticks.min(u32::MAX as u64) as u32);

        self.last_tick_instant = now;

        if target_instant > now {
            // Headroom available: sleep to target instant
            let headroom = target_instant - now;
            self.accumulated_lag = Duration::ZERO;
            TickPacingAction::SleepHeadroom(headroom)
        } else {
            // Engine is behind schedule
            let lag = now - target_instant;
            self.accumulated_lag += lag;
            let lag_micros = lag.as_micros() as u64;
            self.metrics.total_lag_micros =
                self.metrics.total_lag_micros.saturating_add(lag_micros);

            let max_allowed_lag = self
                .config
                .tick_interval
                .saturating_mul(self.config.max_catch_up_ticks);

            if lag > max_allowed_lag {
                // Runaway catch-up detected: clamp lag and advance baseline to prevent death spiral
                self.metrics.catch_up_clamped_events += 1;
                self.baseline_instant = now.checked_sub(max_allowed_lag).unwrap_or(now);
                self.baseline_tick = self
                    .current_tick
                    .saturating_sub(self.config.max_catch_up_ticks as u64);
                self.accumulated_lag = max_allowed_lag;

                TickPacingAction::CatchUpImmediate {
                    lag: max_allowed_lag,
                    clamped: true,
                }
            } else {
                TickPacingAction::CatchUpImmediate {
                    lag,
                    clamped: false,
                }
            }
        }
    }

    /// Sleeps or yields remaining tick headroom in real-time execution.
    pub fn sleep_pacing(&mut self, execution_duration: Duration) -> TickPacingAction {
        let action = self.evaluate_pacing(Instant::now(), execution_duration);
        if let TickPacingAction::SleepHeadroom(headroom) = action {
            thread::sleep(headroom);
        }
        action
    }
}

impl Default for ClockGovernor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_pacing_yields_sleep_headroom() {
        let config = ClockGovernorConfig {
            tick_interval: Duration::from_millis(50),
            suspend_threshold: Duration::from_millis(200),
            max_catch_up_ticks: 4,
        };
        let mut governor = ClockGovernor::with_config(config);
        let start = governor.baseline_instant;

        // Tick 1 execution finishes after 10ms -> 40ms headroom remaining
        let now = start + Duration::from_millis(10);
        let action = governor.evaluate_pacing(now, Duration::from_millis(10));

        assert_eq!(
            action,
            TickPacingAction::SleepHeadroom(Duration::from_millis(40))
        );
        assert_eq!(governor.metrics().total_ticks, 1);
        assert_eq!(governor.metrics().virtualization_pauses_absorbed, 0);
    }

    #[test]
    fn test_virtualization_pause_recovery_and_clamping() {
        let config = ClockGovernorConfig {
            tick_interval: Duration::from_millis(50),
            suspend_threshold: Duration::from_millis(200),
            max_catch_up_ticks: 4,
        };
        let mut governor = ClockGovernor::with_config(config);
        let start = governor.baseline_instant;

        // Simulate normal tick 1
        let now1 = start + Duration::from_millis(50);
        governor.evaluate_pacing(now1, Duration::from_millis(10));

        // Simulate 2000ms hypervisor pause (VM live migration / cloud suspend)
        let now2 = now1 + Duration::from_millis(2000);
        let action = governor.evaluate_pacing(now2, Duration::from_millis(5));

        match action {
            TickPacingAction::VirtualizationResync {
                pause_duration,
                clamped_ticks,
            } => {
                assert_eq!(pause_duration, Duration::from_millis(2000));
                assert_eq!(clamped_ticks, 4);
            }
            _ => panic!("Expected VirtualizationResync, got {:?}", action),
        }

        assert_eq!(governor.metrics().virtualization_pauses_absorbed, 1);
        assert_eq!(
            governor.metrics().max_gap_micros,
            2_000_000 // 2 seconds
        );
    }

    #[test]
    fn test_catch_up_clamping_prevents_spiral_cascades() {
        let config = ClockGovernorConfig {
            tick_interval: Duration::from_millis(50),
            suspend_threshold: Duration::from_millis(500),
            max_catch_up_ticks: 2, // Max 100ms catch up allowed
        };
        let mut governor = ClockGovernor::with_config(config);
        let start = governor.baseline_instant;

        // Advance by 300ms (not triggering 500ms suspend, but exceeding 100ms max allowed lag)
        let now = start + Duration::from_millis(300);
        let action = governor.evaluate_pacing(now, Duration::from_millis(10));

        match action {
            TickPacingAction::CatchUpImmediate { lag, clamped } => {
                assert_eq!(lag, Duration::from_millis(100)); // Clamped to 2 ticks * 50ms
                assert!(clamped);
            }
            _ => panic!("Expected CatchUpImmediate, got {:?}", action),
        }

        assert_eq!(governor.metrics().catch_up_clamped_events, 1);
    }

    #[test]
    fn test_virtualization_resync_subsequent_ticks_pacing() {
        let config = ClockGovernorConfig {
            tick_interval: Duration::from_millis(50),
            suspend_threshold: Duration::from_millis(200),
            max_catch_up_ticks: 4,
        };
        let mut governor = ClockGovernor::with_config(config);
        let start = governor.baseline_instant;

        // Simulate 1,000 normal ticks (50 seconds of continuous uptime)
        let mut current_time = start;
        for _ in 1..=1000 {
            current_time += Duration::from_millis(50);
            governor.evaluate_pacing(current_time, Duration::from_millis(5));
        }
        assert_eq!(governor.current_tick(), 1000);

        // Simulate a 2,000ms hypervisor pause
        let pause_time = current_time + Duration::from_millis(2000);
        let resync_action = governor.evaluate_pacing(pause_time, Duration::from_millis(5));
        assert!(matches!(
            resync_action,
            TickPacingAction::VirtualizationResync { .. }
        ));

        // Evaluate tick 1,002: exactly 50ms after the pause
        let next_tick_time = pause_time + Duration::from_millis(50);
        let post_pause_action = governor.evaluate_pacing(next_tick_time, Duration::from_millis(5));

        // Pacing must be bounded: it must NOT sleep for 50 seconds!
        match post_pause_action {
            TickPacingAction::SleepHeadroom(headroom) => {
                assert!(
                    headroom <= Duration::from_millis(50),
                    "Post-pause headroom must be bounded to tick interval, got {:?}",
                    headroom
                );
            }
            TickPacingAction::CatchUpImmediate { .. } => {
                // Catch-up is also acceptable if behind
            }
            TickPacingAction::VirtualizationResync { .. } => {
                panic!("Should not re-trigger pause resync");
            }
        }
    }
}
