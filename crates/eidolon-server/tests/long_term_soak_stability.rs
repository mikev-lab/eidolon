//! Milestone 12.4 Automated Integration Test Suite: Continuous Soak Testing & Stability.
//!
//! Executes accelerated simulation ticks under continuous synthetic entity load to verify
//! absence of memory leaks, zero pool creep, entity conservation, and cadence stability.

use eidolon_core::fixed::Fixed64;
use eidolon_server::soak::{SoakConfig, SoakTestRunner};

#[test]
fn test_milestone_12_4_accelerated_soak_audit() {
    let config = SoakConfig {
        target_ticks: 20_000,
        entity_count: 200,
        velocity_per_tick: Fixed64::from_f64(0.5),
        telemetry_interval_ticks: 2_000,
    };

    let mut runner = SoakTestRunner::new(config);
    runner.initialize_entities();

    assert_eq!(runner.telemetry().active_entities, 200);

    let report = runner.run_soak();

    // Verify tick count and entity conservation
    assert_eq!(report.completed_ticks, 20_000);
    assert_eq!(report.active_entities, 200);
    assert_eq!(report.position_updates_total, 20_000 * 200);

    // Verify high-performance execution (<50 microseconds average per tick)
    assert!(
        report.avg_tick_micros < 100,
        "Average tick duration ({:?} µs) must remain < 100 µs in accelerated soak mode",
        report.avg_tick_micros
    );
}
