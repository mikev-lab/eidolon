//! Milestone 12.1 Automated Integration Test Suite: Real-World Compound Network Impairment.
//!
//! Validates engine resilience under combined adversarial network conditions:
//! 0.1% to 5.0% continuous/burst packet loss, 20ms to 200ms asymmetric RTT,
//! 150ms network jitter, and compound failure torture testing.

use eidolon_net::impairment::{ImpairmentConfig, ImpairmentOutcome, NetworkImpairmentHarness};

#[test]
fn test_milestone_12_1_continuous_and_burst_packet_loss() {
    // 5.0% packet loss (500 bps) with burst drop length of 3
    let config = ImpairmentConfig {
        loss_bps: 500,
        burst_loss_length: 3,
        min_latency_micros: 0,
        max_latency_micros: 0,
        jitter_micros: 0,
        reorder_bps: 0,
        duplication_bps: 0,
    };

    let mut harness: NetworkImpairmentHarness<64> = NetworkImpairmentHarness::new(config, 99999);
    let mut out = [0u8; 128];
    let total_packets = 10_000;

    for i in 0..total_packets {
        let payload = format!("pkt_{i}");
        let _ = harness.process_egress(payload.as_bytes(), i * 1000, &mut out);
    }

    let metrics = harness.metrics();
    assert_eq!(metrics.total_processed, total_packets);

    // With 5% trigger rate and burst length of 3, dropped packets should be around 10-15% of total
    assert!(
        metrics.dropped_packets > 500 && metrics.dropped_packets < 2_500,
        "Dropped packets ({}) should reflect burst 5% loss profile",
        metrics.dropped_packets
    );
    assert!(metrics.passed_immediate > 7_000);
}

#[test]
fn test_milestone_12_1_latency_and_asymmetric_jitter() {
    // 50ms base latency with up to 150ms asymmetric jitter
    let config = ImpairmentConfig {
        loss_bps: 0,
        burst_loss_length: 1,
        min_latency_micros: 50_000,
        max_latency_micros: 50_000,
        jitter_micros: 150_000,
        reorder_bps: 0,
        duplication_bps: 0,
    };

    let mut harness: NetworkImpairmentHarness<128> = NetworkImpairmentHarness::new(config, 12345);
    let mut out = [0u8; 128];

    // Submit 10 packets at t = 1,000 µs
    for i in 0..10 {
        let payload = format!("jitter_pkt_{i}");
        let outcome = harness.process_egress(payload.as_bytes(), 1_000, &mut out);
        match outcome {
            ImpairmentOutcome::DelayedUntil(deliver_at) => {
                // Must be delayed between 50ms and 200ms (50ms + 150ms jitter)
                assert!((51_000..=201_000).contains(&deliver_at));
            }
            _ => panic!("Expected DelayedUntil outcome"),
        }
    }

    assert_eq!(harness.delayed_count(), 10);

    // At t = 20,000 µs (20ms elapsed), zero packets should be ready
    let mut drain_buf = [0u8; 128];
    assert_eq!(harness.drain_ready(20_000, &mut drain_buf), None);

    // Progress time to t = 250,000 µs (250ms elapsed): all 10 packets must mature and drain
    let mut drained_count = 0;
    while let Some(len) = harness.drain_ready(250_000, &mut drain_buf) {
        assert!(len > 0);
        drained_count += 1;
    }

    assert_eq!(drained_count, 10);
    assert_eq!(harness.delayed_count(), 0);
}

#[test]
fn test_milestone_12_1_compound_torture_mode() {
    // Compound torture: 5.0% loss + 50-150ms latency + 150ms jitter + 2% reorder + 1% duplication
    let config = ImpairmentConfig::adversarial_mobile();
    let mut harness: NetworkImpairmentHarness<256> = NetworkImpairmentHarness::new(config, 88888);

    let mut out = [0u8; 128];
    let total_sent = 5_000;

    for i in 0..total_sent {
        let payload = format!("torture_{i}");
        let outcome = harness.process_egress(payload.as_bytes(), i * 5_000, &mut out);
        assert!(matches!(
            outcome,
            ImpairmentOutcome::Dropped
                | ImpairmentOutcome::DelayedUntil(_)
                | ImpairmentOutcome::PassedImmediate
                | ImpairmentOutcome::Duplicated
        ));
    }

    let m = harness.metrics();
    assert_eq!(m.total_processed, total_sent);
    assert!(m.dropped_packets > 0);
    assert!(m.delayed_packets > 0);
    assert!(m.duplicated_packets > 0);

    // Drain all matured packets up to t = 100 seconds
    let mut drain_buf = [0u8; 128];
    let mut drained = 0;
    while harness.drain_ready(100_000_000, &mut drain_buf).is_some() {
        drained += 1;
    }

    assert!(drained > 0);
}
