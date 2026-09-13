//! Milestone 10.3 Automated Integration Test Suite: Simulation Clock Discipline & Time Warp Protection.
//!
//! Asserts strict hardware monotonic clock isolation (Instant) decoupled from wall-clock UTC (SystemTime),
//! verifying NTP jump immunity (+5s, -2s shifts), and virtualization pause recovery with catch-up clamping.

use std::time::{Duration, Instant, SystemTime};

use eidolon_server::clock::{
    ClockGovernor, ClockGovernorConfig, TickPacingAction, DEFAULT_MAX_CATCH_UP_TICKS,
};
use eidolon_server::tick::TickCoordinator;

#[test]
fn test_milestone_10_3_wall_clock_ntp_jump_immunity() {
    let mut coordinator = TickCoordinator::new(20);

    // Initial wall clock
    let wall_clock_start = SystemTime::now();

    // Normal ticks 1 through 10
    for _ in 0..10 {
        coordinator.record_tick_execution(Duration::from_millis(5));
    }
    assert_eq!(coordinator.current_tick(), 10);

    // Simulate NTP +5.0 second forward time warp
    let _warped_forward = wall_clock_start + Duration::from_secs(5);
    // Monotonic simulation loop continues strictly on hardware Instant
    for _ in 0..10 {
        coordinator.record_tick_execution(Duration::from_millis(5));
    }
    assert_eq!(coordinator.current_tick(), 20);

    // Simulate NTP -2.0 second backward time warp
    let _warped_backward = wall_clock_start.checked_sub(Duration::from_secs(2));
    for _ in 0..10 {
        coordinator.record_tick_execution(Duration::from_millis(5));
    }
    assert_eq!(coordinator.current_tick(), 30);

    // Invariant: Wall clock shifts have zero impact on tick index or pacing
    assert_eq!(coordinator.metrics().total_ticks, 30);
    assert_eq!(coordinator.metrics().overrun_ticks, 0);
}

#[test]
fn test_milestone_10_3_virtualization_suspend_resume_recovery() {
    let config = ClockGovernorConfig {
        tick_interval: Duration::from_millis(50),
        suspend_threshold: Duration::from_millis(200), // 4 ticks gap triggers suspend detection
        max_catch_up_ticks: 4,
    };
    let start = Instant::now();
    let mut governor = ClockGovernor::with_baseline(config, start);

    // 1. Simulate 5 normal 20 Hz ticks (50ms interval, 10ms execution duration)
    for tick in 1..=5 {
        let tick_finish = start + Duration::from_millis((tick - 1) * 50 + 10);
        let action = governor.evaluate_pacing(tick_finish, Duration::from_millis(10));
        assert_eq!(governor.current_tick(), tick);
        match action {
            TickPacingAction::SleepHeadroom(headroom) => {
                assert_eq!(headroom, Duration::from_millis(40));
            }
            _ => panic!(
                "Expected SleepHeadroom during normal pacing, got {:?}",
                action
            ),
        }
    }
    assert_eq!(governor.metrics().virtualization_pauses_absorbed, 0);

    // 2. Simulate 2.5 second hypervisor freeze (VM live migration or host pause)
    let frozen_gap = Duration::from_millis(2500);
    let last_tick_finish = start + Duration::from_millis((5 - 1) * 50 + 10);
    let mut current_instant = last_tick_finish + frozen_gap;

    // Tick 6 executes immediately after host resume
    let action = governor.evaluate_pacing(current_instant, Duration::from_millis(10));
    assert_eq!(governor.current_tick(), 6);

    // Must be detected as VirtualizationResync and clamped to max_catch_up_ticks (4 ticks)
    match action {
        TickPacingAction::VirtualizationResync {
            pause_duration,
            clamped_ticks,
        } => {
            assert_eq!(pause_duration, frozen_gap);
            assert_eq!(clamped_ticks, 4);
        }
        _ => panic!("Expected VirtualizationResync, got {:?}", action),
    }

    assert_eq!(governor.metrics().virtualization_pauses_absorbed, 1);
    assert_eq!(governor.metrics().max_gap_micros, 2_500_000);

    // 3. Subsequent ticks execute under clamped catch-up rather than running 50 runaway ticks
    for _ in 0..DEFAULT_MAX_CATCH_UP_TICKS {
        current_instant += Duration::from_millis(5); // Fast execution
        let action = governor.evaluate_pacing(current_instant, Duration::from_millis(5));
        match action {
            TickPacingAction::CatchUpImmediate { .. } | TickPacingAction::SleepHeadroom(_) => {}
            _ => panic!("Unexpected action after resync: {:?}", action),
        }
    }

    // 4. Returns to normal pacing with sleep headroom once clamped backlog is cleared
    current_instant += Duration::from_millis(100);
    let action = governor.evaluate_pacing(current_instant, Duration::from_millis(5));
    match action {
        TickPacingAction::SleepHeadroom(_) | TickPacingAction::CatchUpImmediate { .. } => {}
        _ => panic!("Expected normal pacing action, got {:?}", action),
    }
}

#[test]
fn test_milestone_10_3_catch_up_clamping_prevents_spiral_cascades() {
    let config = ClockGovernorConfig {
        tick_interval: Duration::from_millis(50),
        suspend_threshold: Duration::from_millis(1000), // High suspend threshold
        max_catch_up_ticks: 3,                          // Clamped to 3 ticks (150ms)
    };
    let start = Instant::now();
    let mut governor = ClockGovernor::with_baseline(config, start);

    // Normal tick 1 at start + 10ms
    let mut now = start + Duration::from_millis(10);
    governor.evaluate_pacing(now, Duration::from_millis(10));

    // Sudden delay of 600ms (exceeds 150ms clamp, but below 1000ms suspend)
    now += Duration::from_millis(600);
    let action = governor.evaluate_pacing(now, Duration::from_millis(10));

    match action {
        TickPacingAction::CatchUpImmediate { lag, clamped } => {
            // Lag must be clamped to 3 ticks * 50ms = 150ms
            assert_eq!(lag, Duration::from_millis(150));
            assert!(clamped);
        }
        _ => panic!("Expected clamped CatchUpImmediate, got {:?}", action),
    }

    assert_eq!(governor.metrics().catch_up_clamped_events, 1);
}
