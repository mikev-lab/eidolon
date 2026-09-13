//! Milestone 11.3 Automated Integration Test Suite: SRE Alerting Rules & Pod Lifecycle.
//!
//! Validates machine-evaluable operational alerting rules:
//! TickBudgetOverrun (p99 > 35ms), CriticalBackpressure, StorageJournalLag, and NetworkPartitionDetected.
//! Validates Agones pod lifecycle supervisor state machine:
//! Ready -> Allocated -> Degraded -> Draining -> Shutdown.

use eidolon_net::backpressure::BackpressureLevel;
use eidolon_server::alerting::{
    AlertConfig, AlertEvaluator, AlertKind, AlertSeverity, PodLifecycleController,
    PodLifecycleState,
};

#[test]
fn test_milestone_11_3_operational_alert_rules_evaluation() {
    let mut evaluator = AlertEvaluator::with_config(AlertConfig {
        max_tick_p99_micros: 35_000,
        tick_overrun_threshold_ticks: 10,
        backpressure_threshold_ticks: 5,
        max_wal_lag_records: 600,
        max_heartbeat_lapses: 3,
    });

    // 1. Tick Budget Overrun Evaluation:
    // Ticks 1..=9 exceed 35,000 µs (threshold is 10 consecutive ticks)
    for tick in 1..=9 {
        evaluator.evaluate_tick(tick, 38_000, BackpressureLevel::Normal, 0, 0);
        assert_eq!(evaluator.active_alert_count(), 0);
    }
    // 10th consecutive tick triggers Critical alert
    evaluator.evaluate_tick(10, 38_000, BackpressureLevel::Normal, 0, 0);
    assert_eq!(evaluator.active_alert_count(), 1);
    assert!(evaluator.has_alert_at_least(AlertSeverity::Critical));
    assert_eq!(
        evaluator.active_alerts()[0].unwrap().kind,
        AlertKind::TickBudgetOverrun
    );

    // Healthy tick clears overrun alert
    evaluator.evaluate_tick(11, 14_000, BackpressureLevel::Normal, 0, 0);
    assert_eq!(evaluator.active_alert_count(), 0);

    // 2. Critical Backpressure Evaluation:
    // 4 ticks at Critical backpressure
    for tick in 12..=15 {
        evaluator.evaluate_tick(tick, 10_000, BackpressureLevel::Critical, 0, 0);
        assert_eq!(evaluator.active_alert_count(), 0);
    }
    // 5th consecutive tick triggers alert
    evaluator.evaluate_tick(16, 10_000, BackpressureLevel::Critical, 0, 0);
    assert_eq!(evaluator.active_alert_count(), 1);
    assert_eq!(
        evaluator.active_alerts()[0].unwrap().kind,
        AlertKind::CriticalBackpressure
    );

    // 3. Storage Journal Lag Evaluation:
    // WAL lag of 650 records (threshold 600)
    evaluator.evaluate_tick(17, 10_000, BackpressureLevel::Normal, 650, 0);
    assert_eq!(evaluator.active_alert_count(), 1);
    assert_eq!(
        evaluator.active_alerts()[0].unwrap().kind,
        AlertKind::StorageJournalLag
    );

    // 4. Network Partition Split Brain Evaluation:
    // Heartbeat lapses = 3 (Fatal severity)
    evaluator.evaluate_tick(18, 10_000, BackpressureLevel::Normal, 0, 3);
    assert_eq!(evaluator.active_alert_count(), 1);
    assert!(evaluator.has_alert_at_least(AlertSeverity::Fatal));
    assert_eq!(
        evaluator.active_alerts()[0].unwrap().kind,
        AlertKind::NetworkPartitionDetected
    );
}

#[test]
fn test_milestone_11_3_pod_lifecycle_state_machine() {
    let mut controller = PodLifecycleController::new();
    assert_eq!(controller.state(), PodLifecycleState::Starting);

    // Step 1: Initialize server and mark Ready
    controller.mark_ready();
    assert_eq!(controller.state(), PodLifecycleState::Ready);

    // Step 2: Accept incoming sessions and mark Allocated
    controller.mark_allocated();
    assert_eq!(controller.state(), PodLifecycleState::Allocated);

    let mut evaluator = AlertEvaluator::default();

    // Step 3: Sustained Critical alert degrades pod performance state
    for tick in 1..=10 {
        evaluator.evaluate_tick(tick, 40_000, BackpressureLevel::Normal, 0, 0);
    }
    let state = controller.update(&evaluator);
    assert_eq!(state, PodLifecycleState::Degraded);

    // Step 4: Condition resolves, recovers to Allocated
    evaluator.evaluate_tick(11, 10_000, BackpressureLevel::Normal, 0, 0);
    let state = controller.update(&evaluator);
    assert_eq!(state, PodLifecycleState::Allocated);

    // Step 5: Fatal network partition triggers automatic graceful Draining
    evaluator.evaluate_tick(12, 10_000, BackpressureLevel::Normal, 0, 3);
    let state = controller.update(&evaluator);
    assert_eq!(state, PodLifecycleState::Draining);

    // Step 6: Drain completes after 100 ticks (5 seconds at 20 Hz) -> Shutdown
    for _ in 0..100 {
        controller.update(&evaluator);
    }
    assert_eq!(controller.state(), PodLifecycleState::Shutdown);
}

#[test]
fn test_milestone_11_3_manual_sigterm_drain_runbook() {
    let mut controller = PodLifecycleController::new();
    controller.mark_ready();
    controller.mark_allocated();
    assert_eq!(controller.state(), PodLifecycleState::Allocated);

    // SRE or Agones sends SIGTERM trigger
    controller.initiate_drain();
    assert_eq!(controller.state(), PodLifecycleState::Draining);

    let evaluator = AlertEvaluator::default();
    for _ in 0..100 {
        controller.update(&evaluator);
    }
    assert_eq!(controller.state(), PodLifecycleState::Shutdown);
}
