//! Milestone 10.1 Automated Integration Test Suite: Unified End-to-End Backpressure Pipeline.
//!
//! Asserts continuous backpressure flow (Socket -> Ingress -> Sim -> AoI -> Egress -> Socket),
//! verifying zero unbounded queues, stale unsequenced movement dropping under ingress pressure,
//! oldest-unreliable dropping under egress backpressure, and strict preservation of reliable packets.

use eidolon_net::backpressure::{
    BackpressureCoordinator, BackpressureLevel, BoundedEgressQueue, BoundedIngressQueue,
};
use eidolon_net::error::NetError;
use eidolon_spatial::aoi::{AoIScheduler, LoadSheddingLevel};
use eidolon_spatial::tier::FrequencyTier;

#[test]
fn test_milestone_10_1_continuous_pipeline_flow_under_normal_load() {
    // Stage 1: Ingress Queue (Socket -> Sim)
    let mut ingress: BoundedIngressQueue<u32, 16> = BoundedIngressQueue::new();
    // Stage 2: Egress Queue (Sim -> Socket)
    let mut egress: BoundedEgressQueue<u32, 16> = BoundedEgressQueue::new();
    let mut coordinator = BackpressureCoordinator::new();

    // Push normal mix of movement and combat packets
    for seq in 1..=5 {
        assert!(ingress.push(seq * 10, false, seq as u16).is_ok());
    }
    assert!(ingress.push(999, true, 6).is_ok()); // 1 reliable packet

    assert_eq!(ingress.len(), 6);
    assert_eq!(ingress.backpressure_level(), BackpressureLevel::Normal);

    // Update coordinator with current utilization
    let level = coordinator.update_pipeline(
        ingress.utilization_pct(),
        20, // Sim 20%
        egress.utilization_pct(),
    );
    assert_eq!(level, BackpressureLevel::Normal);
    assert!(!coordinator.should_shed_horizon());
    assert!(!coordinator.should_shed_mid_range());

    // Simulation drains ingress and generates egress updates
    let mut drained = [None; 8];
    let drained_count = ingress.drain_into(&mut drained);
    assert_eq!(drained_count, 6);

    for pkt in drained.iter().take(drained_count).flatten() {
        assert!(egress
            .push(pkt.payload, pkt.is_reliable, pkt.sequence)
            .is_ok());
    }

    assert_eq!(egress.len(), 6);
    assert_eq!(egress.backpressure_level(), BackpressureLevel::Normal);
}

#[test]
fn test_milestone_10_1_ingress_saturation_drops_stale_preserves_reliable() {
    // Ingress queue with capacity 8
    let mut ingress: BoundedIngressQueue<u32, 8> = BoundedIngressQueue::new();

    // Fill ingress queue to 100% capacity with unreliable movement packets (seq 1..=8)
    for seq in 1..=8 {
        assert!(ingress.push(seq * 100, false, seq as u16).is_ok());
    }
    assert!(ingress.is_full());
    assert_eq!(ingress.utilization_pct(), 100);
    assert_eq!(ingress.backpressure_level(), BackpressureLevel::Critical);

    // Attempting to push a 9th unreliable movement packet is rejected (dropped as stale movement)
    assert_eq!(ingress.push(900, false, 9), Err(NetError::QueueFull));
    assert_eq!(ingress.metrics().ingress_stale_dropped, 1);

    // Incoming critical reliable trade transaction (is_reliable = true, seq 10)
    // Must succeed by evicting the oldest unreliable packet (seq 1, value 100)
    assert!(ingress.push(10000, true, 10).is_ok());
    assert_eq!(ingress.len(), 8);
    assert_eq!(ingress.metrics().ingress_stale_dropped, 2);

    // Another reliable packet (seq 11) evicts seq 2 (value 200)
    assert!(ingress.push(11000, true, 11).is_ok());
    assert_eq!(ingress.len(), 8);
    assert_eq!(ingress.metrics().ingress_stale_dropped, 3);

    // Verify queue ordering and preservation:
    // Evicted: 100, 200
    // Remaining in FIFO order: 300, 400, 500, 600, 700, 800, 10000 (reliable), 11000 (reliable)
    let p1 = ingress.pop().unwrap();
    assert_eq!(p1.payload, 300);
    assert!(!p1.is_reliable);

    let p2 = ingress.pop().unwrap();
    assert_eq!(p2.payload, 400);

    let p3 = ingress.pop().unwrap();
    assert_eq!(p3.payload, 500);

    let p4 = ingress.pop().unwrap();
    assert_eq!(p4.payload, 600);

    let p5 = ingress.pop().unwrap();
    assert_eq!(p5.payload, 700);

    let p6 = ingress.pop().unwrap();
    assert_eq!(p6.payload, 800);

    let p7 = ingress.pop().unwrap();
    assert_eq!(p7.payload, 10000);
    assert!(p7.is_reliable);

    let p8 = ingress.pop().unwrap();
    assert_eq!(p8.payload, 11000);
    assert!(p8.is_reliable);

    assert!(ingress.is_empty());
}

#[test]
fn test_milestone_10_1_egress_saturation_drops_oldest_unreliable_for_freshest() {
    // Egress queue with capacity 4
    let mut egress: BoundedEgressQueue<u32, 4> = BoundedEgressQueue::new();

    // Enqueue 1 reliable state packet + 3 unreliable position updates
    assert!(egress.push(1000, true, 1).is_ok());
    assert!(egress.push(10, false, 2).is_ok());
    assert!(egress.push(20, false, 3).is_ok());
    assert!(egress.push(30, false, 4).is_ok());
    assert!(egress.is_full());

    // Simulation produces fresher position update 40 (seq 5)
    // Under egress backpressure: drops oldest unreliable transform (10), preserves reliable 1000
    assert!(egress.push(40, false, 5).is_ok());
    assert_eq!(egress.len(), 4);
    assert_eq!(egress.metrics().egress_unreliable_dropped, 1);

    // Simulation produces fresher position update 50 (seq 6)
    // Drops oldest remaining unreliable transform (20)
    assert!(egress.push(50, false, 6).is_ok());
    assert_eq!(egress.len(), 4);
    assert_eq!(egress.metrics().egress_unreliable_dropped, 2);

    // Verified contents:
    // 1000 (reliable) strictly preserved
    // 30, 40, 50 (newest positions)
    let p1 = egress.pop().unwrap();
    assert_eq!(p1.payload, 1000);
    assert!(p1.is_reliable);

    let p2 = egress.pop().unwrap();
    assert_eq!(p2.payload, 30);
    assert!(!p2.is_reliable);

    let p3 = egress.pop().unwrap();
    assert_eq!(p3.payload, 40);
    assert!(!p3.is_reliable);

    let p4 = egress.pop().unwrap();
    assert_eq!(p4.payload, 50);
    assert!(!p4.is_reliable);

    assert!(egress.is_empty());
}

#[test]
fn test_milestone_10_1_multi_stage_backpressure_and_aoi_coordination() {
    let mut ingress: BoundedIngressQueue<u32, 100> = BoundedIngressQueue::new();
    let mut egress: BoundedEgressQueue<u32, 100> = BoundedEgressQueue::new();
    let mut coordinator = BackpressureCoordinator::new();
    let mut aoi_scheduler = AoIScheduler::new();

    // 1. Initial baseline: 10% utilization -> Normal
    for seq in 1..=10 {
        let _ = ingress.push(seq, false, seq as u16);
    }
    let level =
        coordinator.update_pipeline(ingress.utilization_pct(), 15, egress.utilization_pct());
    assert_eq!(level, BackpressureLevel::Normal);
    assert!(!coordinator.should_shed_horizon());

    // 2. Ingress spikes to 65% -> Elevated backpressure -> Shed Horizon
    for seq in 11..=65 {
        let _ = ingress.push(seq, false, seq as u16);
    }
    let level =
        coordinator.update_pipeline(ingress.utilization_pct(), 30, egress.utilization_pct());
    assert_eq!(level, BackpressureLevel::Elevated);
    assert!(coordinator.should_shed_horizon());
    assert!(!coordinator.should_shed_mid_range());

    // Coordinate with AoI scheduler: set Level 1 load shedding
    if coordinator.should_shed_horizon() {
        aoi_scheduler.set_load_shedding(LoadSheddingLevel::Level1);
    }
    // Horizon is shed under Level 1
    assert!(!aoi_scheduler.should_replicate(FrequencyTier::Horizon, 20, true));
    // Mid tier throttled to 1 Hz (once every 20 ticks at 20 Hz)
    assert!(aoi_scheduler.should_replicate(FrequencyTier::Mid, 20, false));
    assert!(!aoi_scheduler.should_replicate(FrequencyTier::Mid, 10, false));

    // 3. Egress spikes to 85% -> Saturated backpressure -> Shed Mid tier
    for seq in 1..=85 {
        let _ = egress.push(seq, false, seq as u16);
    }
    let level =
        coordinator.update_pipeline(ingress.utilization_pct(), 30, egress.utilization_pct());
    assert_eq!(level, BackpressureLevel::Saturated);
    assert!(coordinator.should_shed_mid_range());

    if coordinator.should_shed_mid_range() {
        aoi_scheduler.set_load_shedding(LoadSheddingLevel::Level2);
    }
    // Level 2 drops Mid and Horizon tiers entirely, protecting Immediate combat
    assert!(!aoi_scheduler.should_replicate(FrequencyTier::Mid, 20, false));
    assert!(!aoi_scheduler.should_replicate(FrequencyTier::Horizon, 20, true));
    assert!(aoi_scheduler.should_replicate(FrequencyTier::Immediate, 20, false));
}
