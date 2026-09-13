//! Milestone 10.2 Automated Integration Test Suite: Hierarchical Capacity Admission Control.
//!
//! Asserts priority shedding hierarchy:
//! Critical Combat Events > Nearby Movement (<10m) > Mid-Range Movement (<50m) > Far State > Cosmetics.
//! Verifies frequency decimation, bandwidth clamping, and visual continuity under saturation paging.

use eidolon_net::admission::{AdmissionConfig, AdmissionController, PriorityClass};
use eidolon_net::backpressure::BackpressureLevel;

#[test]
fn test_milestone_10_2_priority_shedding_hierarchy_order() {
    let mut controller = AdmissionController::with_config(AdmissionConfig {
        max_bytes_per_tick: 200,
        max_packets_per_tick: 10,
        reserved_critical_bytes: 40,
    });

    controller.advance_tick(1);

    // 1. Normal pressure: all traffic classes admitted
    assert!(controller.try_admit(PriorityClass::Cosmetic, 20, BackpressureLevel::Normal));
    assert!(controller.try_admit(PriorityClass::FarState, 20, BackpressureLevel::Normal));
    assert!(controller.try_admit(
        PriorityClass::MidRangeMovement,
        20,
        BackpressureLevel::Normal
    ));
    assert!(controller.try_admit(
        PriorityClass::ImmediateMovement,
        20,
        BackpressureLevel::Normal
    ));
    assert!(controller.try_admit(PriorityClass::CriticalCombat, 30, BackpressureLevel::Normal));

    // Reset tick
    controller.advance_tick(2);

    // 2. Elevated pressure: Cosmetic shed first (Priority 4), all higher priorities admitted
    assert!(!controller.try_admit(PriorityClass::Cosmetic, 20, BackpressureLevel::Elevated));
    assert!(controller.try_admit(PriorityClass::FarState, 20, BackpressureLevel::Elevated));
    assert!(controller.try_admit(
        PriorityClass::MidRangeMovement,
        20,
        BackpressureLevel::Elevated
    ));
    assert!(controller.try_admit(
        PriorityClass::ImmediateMovement,
        20,
        BackpressureLevel::Elevated
    ));
    assert!(controller.try_admit(
        PriorityClass::CriticalCombat,
        30,
        BackpressureLevel::Elevated
    ));
    assert_eq!(controller.metrics().shed_cosmetic, 1);

    // Reset tick
    controller.advance_tick(3);

    // 3. Saturated pressure: Cosmetic and FarState shed, MidRange decimated on odd tick
    assert!(!controller.try_admit(PriorityClass::Cosmetic, 20, BackpressureLevel::Saturated));
    assert!(!controller.try_admit(PriorityClass::FarState, 20, BackpressureLevel::Saturated));
    // Odd tick (3): MidRange decimated
    assert!(!controller.try_admit(
        PriorityClass::MidRangeMovement,
        20,
        BackpressureLevel::Saturated
    ));
    // Immediate and Critical admitted
    assert!(controller.try_admit(
        PriorityClass::ImmediateMovement,
        20,
        BackpressureLevel::Saturated
    ));
    assert!(controller.try_admit(
        PriorityClass::CriticalCombat,
        30,
        BackpressureLevel::Saturated
    ));

    // Reset tick to even tick 4
    controller.advance_tick(4);
    // Even tick (4): MidRange admitted under Saturated
    assert!(controller.try_admit(
        PriorityClass::MidRangeMovement,
        20,
        BackpressureLevel::Saturated
    ));

    // Reset tick
    controller.advance_tick(5);

    // 4. Critical pressure: Cosmetic, FarState, and MidRange shed completely
    assert!(!controller.try_admit(PriorityClass::Cosmetic, 20, BackpressureLevel::Critical));
    assert!(!controller.try_admit(PriorityClass::FarState, 20, BackpressureLevel::Critical));
    assert!(!controller.try_admit(
        PriorityClass::MidRangeMovement,
        20,
        BackpressureLevel::Critical
    ));
    // Odd tick (5): Immediate decimated
    assert!(!controller.try_admit(
        PriorityClass::ImmediateMovement,
        20,
        BackpressureLevel::Critical
    ));
    // Critical Combat is ALWAYS admitted
    assert!(controller.try_admit(
        PriorityClass::CriticalCombat,
        40,
        BackpressureLevel::Critical
    ));
    assert!(controller.try_admit(
        PriorityClass::CriticalCombat,
        40,
        BackpressureLevel::Critical
    ));
}

#[test]
fn test_milestone_10_2_bandwidth_budget_clamping_and_critical_protection() {
    // 100 bytes budget per tick, 30 bytes reserved for CriticalCombat
    let mut controller = AdmissionController::with_config(AdmissionConfig {
        max_bytes_per_tick: 100,
        max_packets_per_tick: 8,
        reserved_critical_bytes: 30, // General budget = 70 bytes
    });

    controller.advance_tick(1);

    // Admit 50 bytes of immediate movement (remaining general budget = 20 bytes)
    assert!(controller.try_admit(
        PriorityClass::ImmediateMovement,
        50,
        BackpressureLevel::Normal
    ));
    assert_eq!(controller.remaining_general_budget(), 20);

    // Attempting to admit 30 bytes of far state exceeds 20 bytes general budget -> shed
    assert!(!controller.try_admit(PriorityClass::FarState, 30, BackpressureLevel::Normal));
    assert_eq!(controller.metrics().shed_far_state, 1);

    // Attempting to admit 25 bytes of cosmetic exceeds 20 bytes general budget -> shed
    assert!(!controller.try_admit(PriorityClass::Cosmetic, 25, BackpressureLevel::Normal));
    assert_eq!(controller.metrics().shed_cosmetic, 1);

    // Admitting 20 bytes of mid-range fits exactly in remaining general budget
    assert!(controller.try_admit(
        PriorityClass::MidRangeMovement,
        20,
        BackpressureLevel::Normal
    ));
    assert_eq!(controller.remaining_general_budget(), 0);

    // General budget is now 0; any further non-critical traffic is rejected
    assert!(!controller.try_admit(
        PriorityClass::ImmediateMovement,
        10,
        BackpressureLevel::Normal
    ));
    assert_eq!(controller.metrics().shed_immediate, 1);

    // Critical Combat is admitted into its dedicated reserved budget
    assert!(controller.try_admit(PriorityClass::CriticalCombat, 25, BackpressureLevel::Normal));
    assert_eq!(controller.metrics().admitted_critical, 1);
}

#[test]
fn test_milestone_10_2_visual_continuity_under_saturation_paging() {
    // Simulates an observer receiving updates under varying pressure over 10 consecutive ticks
    let mut controller = AdmissionController::with_config(AdmissionConfig {
        max_bytes_per_tick: 150,
        max_packets_per_tick: 5,
        reserved_critical_bytes: 30,
    });

    let mut critical_combat_received = 0;
    let mut immediate_movement_received = 0;

    for tick in 1..=10 {
        controller.advance_tick(tick);

        // Under Saturated pressure
        let pressure = BackpressureLevel::Saturated;

        // Every tick: 1 critical combat action (e.g. boss strike or spell)
        if controller.try_admit(PriorityClass::CriticalCombat, 15, pressure) {
            critical_combat_received += 1;
        }

        // Every tick: 1 immediate nearby transform (e.g. boss or teammate movement)
        if controller.try_admit(PriorityClass::ImmediateMovement, 11, pressure) {
            immediate_movement_received += 1;
        }

        // Distant cosmetics and horizon entities are paged out
        let _ = controller.try_admit(PriorityClass::Cosmetic, 8, pressure);
        let _ = controller.try_admit(PriorityClass::FarState, 15, pressure);
    }

    // Critical Combat must have 100% delivery across all 10 ticks
    assert_eq!(critical_combat_received, 10);
    // Immediate movement within 10m maintains 100% responsiveness under Saturated pressure
    assert_eq!(immediate_movement_received, 10);
    // All cosmetics and far states were safely shed
    assert_eq!(controller.metrics().shed_cosmetic, 10);
    assert_eq!(controller.metrics().shed_far_state, 10);
}
