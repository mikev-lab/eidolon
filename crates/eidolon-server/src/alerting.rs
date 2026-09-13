//! SRE operational alerting rules, health evaluation, and Agones pod lifecycle draining.
//!
//! Evaluates cluster alerts (tick budget overruns, storage journal lag, critical backpressure,
//! and network partitions) and orchestrates automated Agones pod lifecycle transitions
//! (Ready -> Allocated -> Degraded -> Draining -> Shutdown).

use eidolon_net::backpressure::BackpressureLevel;

/// Severity classification for operational cluster alerts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertSeverity {
    /// Warning: Non-fatal degradation requiring observation.
    Warning = 1,
    /// Critical: Severe performance impact; triggers automatic pod draining if sustained.
    Critical = 2,
    /// Fatal: Unrecoverable split-brain or data corruption; triggers immediate pod drainage.
    Fatal = 3,
}

/// Identifiers for automated operational alerting rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertKind {
    /// p99 tick duration exceeds 35,000 microseconds for >10 consecutive ticks.
    TickBudgetOverrun,
    /// Queue backpressure remains at Critical for >5 consecutive ticks.
    CriticalBackpressure,
    /// Unflushed WAL records exceed 600 entries (storage database lag).
    StorageJournalLag,
    /// Inter-zone boundary link heartbeat has lapsed for >3 cycles.
    NetworkPartitionDetected,
    /// Per-connection memory utilization exceeds 85% of configured ceiling.
    MemoryCeilingApproaching,
}

/// An active alert instance with onset metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveAlert {
    /// Kind of alert.
    pub kind: AlertKind,
    /// Assigned severity.
    pub severity: AlertSeverity,
    /// Simulation tick at which the alert first triggered.
    pub onset_tick: u64,
    /// Number of consecutive ticks this condition has persisted.
    pub consecutive_ticks: u32,
}

/// Configuration thresholds for operational alert triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlertConfig {
    /// Maximum allowable p99 tick duration in microseconds (default: 35,000 µs / 70% budget).
    pub max_tick_p99_micros: u64,
    /// Consecutive ticks exceeding tick threshold to trigger alert (default: 10).
    pub tick_overrun_threshold_ticks: u32,
    /// Consecutive ticks at Critical backpressure to trigger alert (default: 5).
    pub backpressure_threshold_ticks: u32,
    /// Maximum allowed lag between current and durable WAL LSN (default: 600 records).
    pub max_wal_lag_records: u64,
    /// Heartbeat lapses triggering network partition alarm (default: 3).
    pub max_heartbeat_lapses: u32,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            max_tick_p99_micros: 35_000,
            tick_overrun_threshold_ticks: 10,
            backpressure_threshold_ticks: 5,
            max_wal_lag_records: 600,
            max_heartbeat_lapses: 3,
        }
    }
}

/// Real-time evaluator monitoring operational conditions and evaluating alert rules.
#[derive(Debug)]
pub struct AlertEvaluator {
    config: AlertConfig,
    consecutive_tick_overruns: u32,
    consecutive_critical_backpressure: u32,
    active_alerts: [Option<ActiveAlert>; 5],
    active_alert_count: usize,
}

impl AlertEvaluator {
    /// Creates a new alert evaluator with default configuration.
    pub fn new() -> Self {
        Self::with_config(AlertConfig::default())
    }

    /// Creates a new alert evaluator with custom configuration.
    pub fn with_config(config: AlertConfig) -> Self {
        Self {
            config,
            consecutive_tick_overruns: 0,
            consecutive_critical_backpressure: 0,
            active_alerts: [None; 5],
            active_alert_count: 0,
        }
    }

    /// Evaluates current operational metrics and updates active alert states.
    pub fn evaluate_tick(
        &mut self,
        tick: u64,
        tick_p99_micros: u64,
        backpressure: BackpressureLevel,
        wal_lag: u64,
        heartbeat_lapses: u32,
    ) {
        self.active_alert_count = 0;
        for slot in self.active_alerts.iter_mut() {
            *slot = None;
        }

        // 1. Tick Budget Overrun
        if tick_p99_micros >= self.config.max_tick_p99_micros {
            self.consecutive_tick_overruns += 1;
            if self.consecutive_tick_overruns >= self.config.tick_overrun_threshold_ticks {
                self.record_alert(ActiveAlert {
                    kind: AlertKind::TickBudgetOverrun,
                    severity: AlertSeverity::Critical,
                    onset_tick: tick.saturating_sub(self.consecutive_tick_overruns as u64),
                    consecutive_ticks: self.consecutive_tick_overruns,
                });
            }
        } else {
            self.consecutive_tick_overruns = 0;
        }

        // 2. Critical Backpressure
        if backpressure == BackpressureLevel::Critical {
            self.consecutive_critical_backpressure += 1;
            if self.consecutive_critical_backpressure >= self.config.backpressure_threshold_ticks {
                self.record_alert(ActiveAlert {
                    kind: AlertKind::CriticalBackpressure,
                    severity: AlertSeverity::Critical,
                    onset_tick: tick.saturating_sub(self.consecutive_critical_backpressure as u64),
                    consecutive_ticks: self.consecutive_critical_backpressure,
                });
            }
        } else {
            self.consecutive_critical_backpressure = 0;
        }

        // 3. Storage Journal Lag
        if wal_lag >= self.config.max_wal_lag_records {
            self.record_alert(ActiveAlert {
                kind: AlertKind::StorageJournalLag,
                severity: AlertSeverity::Critical,
                onset_tick: tick,
                consecutive_ticks: 1,
            });
        }

        // 4. Network Partition Split Brain
        if heartbeat_lapses >= self.config.max_heartbeat_lapses {
            self.record_alert(ActiveAlert {
                kind: AlertKind::NetworkPartitionDetected,
                severity: AlertSeverity::Fatal,
                onset_tick: tick,
                consecutive_ticks: heartbeat_lapses,
            });
        }
    }

    /// Returns the number of currently active alerts.
    #[inline]
    pub const fn active_alert_count(&self) -> usize {
        self.active_alert_count
    }

    /// Returns true if any alert of the specified severity is active.
    pub fn has_alert_at_least(&self, severity: AlertSeverity) -> bool {
        for i in 0..self.active_alert_count {
            if let Some(ref alert) = self.active_alerts[i] {
                if alert.severity >= severity {
                    return true;
                }
            }
        }
        false
    }

    /// Returns the active alerts slice.
    pub fn active_alerts(&self) -> &[Option<ActiveAlert>] {
        &self.active_alerts[..self.active_alert_count]
    }

    fn record_alert(&mut self, alert: ActiveAlert) {
        if self.active_alert_count < self.active_alerts.len() {
            self.active_alerts[self.active_alert_count] = Some(alert);
            self.active_alert_count += 1;
        }
    }
}

impl Default for AlertEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

/// Lifecycle states for an Agones Kubernetes game server pod.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PodLifecycleState {
    /// Process starting up, binding sockets, and allocating static pools.
    #[default]
    Starting = 0,
    /// Warm and ready for Agones player assignment.
    Ready = 1,
    /// Actively hosting player sessions and simulated zones.
    Allocated = 2,
    /// Operating under degraded performance or sustained alerts.
    Degraded = 3,
    /// Gracefully draining active sessions, migrating entities, and serializing state.
    Draining = 4,
    /// State completely flushed; ready for clean process termination.
    Shutdown = 5,
}

/// Automated Agones pod lifecycle supervisor managing state transitions and drain runbooks.
#[derive(Debug)]
pub struct PodLifecycleController {
    state: PodLifecycleState,
    draining_ticks: u32,
    max_drain_ticks: u32,
}

impl PodLifecycleController {
    /// Creates a new pod lifecycle controller.
    pub fn new() -> Self {
        Self {
            state: PodLifecycleState::Starting,
            draining_ticks: 0,
            max_drain_ticks: 100, // 5 seconds at 20 Hz
        }
    }

    /// Returns the current pod lifecycle state.
    #[inline]
    pub const fn state(&self) -> PodLifecycleState {
        self.state
    }

    /// Transitions to Ready state once engine initialization completes.
    pub fn mark_ready(&mut self) {
        if self.state == PodLifecycleState::Starting {
            self.state = PodLifecycleState::Ready;
        }
    }

    /// Transitions to Allocated state when player sessions connect.
    pub fn mark_allocated(&mut self) {
        if self.state == PodLifecycleState::Ready {
            self.state = PodLifecycleState::Allocated;
        }
    }

    /// Evaluates operational alerts and orchestrates automated degradation and draining.
    pub fn update(&mut self, evaluator: &AlertEvaluator) -> PodLifecycleState {
        match self.state {
            PodLifecycleState::Allocated => {
                if evaluator.has_alert_at_least(AlertSeverity::Fatal) {
                    // Immediate drain on fatal split brain or corruption
                    self.state = PodLifecycleState::Draining;
                    self.draining_ticks = 0;
                } else if evaluator.has_alert_at_least(AlertSeverity::Critical) {
                    self.state = PodLifecycleState::Degraded;
                }
            }
            PodLifecycleState::Degraded => {
                if evaluator.has_alert_at_least(AlertSeverity::Fatal) {
                    self.state = PodLifecycleState::Draining;
                    self.draining_ticks = 0;
                } else if !evaluator.has_alert_at_least(AlertSeverity::Warning) {
                    // Recover back to Allocated
                    self.state = PodLifecycleState::Allocated;
                }
            }
            PodLifecycleState::Draining => {
                self.draining_ticks += 1;
                if self.draining_ticks >= self.max_drain_ticks {
                    self.state = PodLifecycleState::Shutdown;
                }
            }
            _ => {}
        }
        self.state
    }

    /// Explicitly initiates graceful draining (e.g. upon SIGTERM trap from Agones eviction).
    pub fn initiate_drain(&mut self) {
        if self.state != PodLifecycleState::Shutdown {
            self.state = PodLifecycleState::Draining;
            self.draining_ticks = 0;
        }
    }
}

impl Default for PodLifecycleController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alert_evaluator_tick_overrun_and_recovery() {
        let mut evaluator = AlertEvaluator::with_config(AlertConfig {
            max_tick_p99_micros: 35_000,
            tick_overrun_threshold_ticks: 3,
            ..AlertConfig::default()
        });

        // 2 overrun ticks: not yet triggered
        evaluator.evaluate_tick(1, 40_000, BackpressureLevel::Normal, 0, 0);
        evaluator.evaluate_tick(2, 40_000, BackpressureLevel::Normal, 0, 0);
        assert_eq!(evaluator.active_alert_count(), 0);

        // 3rd overrun tick: triggers Critical alert
        evaluator.evaluate_tick(3, 40_000, BackpressureLevel::Normal, 0, 0);
        assert_eq!(evaluator.active_alert_count(), 1);
        assert!(evaluator.has_alert_at_least(AlertSeverity::Critical));

        // Fast tick: clears overrun alert
        evaluator.evaluate_tick(4, 15_000, BackpressureLevel::Normal, 0, 0);
        assert_eq!(evaluator.active_alert_count(), 0);
        assert!(!evaluator.has_alert_at_least(AlertSeverity::Warning));
    }

    #[test]
    fn test_pod_lifecycle_controller_fatal_drain() {
        let mut controller = PodLifecycleController::new();
        controller.mark_ready();
        controller.mark_allocated();
        assert_eq!(controller.state(), PodLifecycleState::Allocated);

        let mut evaluator = AlertEvaluator::default();

        // Induce fatal network partition (heartbeats = 4)
        evaluator.evaluate_tick(100, 10_000, BackpressureLevel::Normal, 0, 4);
        assert!(evaluator.has_alert_at_least(AlertSeverity::Fatal));

        let state = controller.update(&evaluator);
        assert_eq!(state, PodLifecycleState::Draining);

        // Progress drain to completion
        for _ in 0..100 {
            controller.update(&evaluator);
        }
        assert_eq!(controller.state(), PodLifecycleState::Shutdown);
    }
}
