//! Milestone 11.1 Automated Integration Test Suite: Prometheus Metrics, SLOs & Bounded Cardinality.
//!
//! Validates fixed-memory logarithmic latency histograms (p50, p95, p99, p99.9), microsecond
//! phase attribution, network throughput counters, and Prometheus text exposition formatting
//! under strict bounded-cardinality invariants.

use eidolon_server::metrics::{
    LatencyHistogram, PrometheusExporter, ServerMetrics, ZoneMetrics, DEFAULT_TICK_BUCKETS_MICROS,
};

#[test]
fn test_milestone_11_1_latency_histogram_percentile_precision() {
    let mut hist: LatencyHistogram<16> = LatencyHistogram::new(DEFAULT_TICK_BUCKETS_MICROS);

    // Populate 1,000 synthetic observations representing a healthy 20 Hz simulation with occasional tail spikes:
    // - 800 observations in 1,000..=4,500 µs (<= 5,000 bucket)
    // - 150 observations in 10,000..=18,000 µs (<= 20,000 bucket)
    // - 40 observations in 25,000..=32,000 µs (<= 35,000 bucket)
    // - 10 observations in 42,000..=48,000 µs (<= 50,000 bucket)
    for _ in 0..800 {
        hist.record(3_200);
    }
    for _ in 0..150 {
        hist.record(15_000);
    }
    for _ in 0..40 {
        hist.record(32_000);
    }
    for _ in 0..10 {
        hist.record(46_000);
    }

    assert_eq!(hist.count(), 1000);
    assert_eq!(hist.min_micros(), 3_200);
    assert_eq!(hist.max_micros(), 46_000);

    let expected_sum: u64 = (800 * 3_200) + (150 * 15_000) + (40 * 32_000) + (10 * 46_000);
    assert_eq!(hist.sum_micros(), expected_sum);

    // Verify percentile SLO bucket classifications:
    // p50 rank = 500th observation -> bucket 5_000 µs
    assert_eq!(hist.percentile(0.50), 5_000);
    // p95 rank = 950th observation (800 + 150 = 950) -> bucket 20_000 µs
    assert_eq!(hist.percentile(0.95), 20_000);
    // p99 rank = 990th observation (950 + 40 = 990) -> bucket 35_000 µs
    assert_eq!(hist.percentile(0.99), 35_000);
    // p99.9 rank = 999th observation -> bucket 48_000 µs (since 46_000 <= 48_000)
    assert_eq!(hist.percentile(0.999), 48_000);
}

#[test]
fn test_milestone_11_1_multi_phase_and_multi_zone_metrics() {
    let mut server = ServerMetrics::default();

    // Populate latency observations
    for _ in 0..500 {
        server.tick_histogram.record(4_200);
    }

    // Populate phase attribution
    server.phase_ingress_micros = 1_150;
    server.phase_sim_micros = 850;
    server.phase_spatial_micros = 2_100;
    server.phase_egress_micros = 620;

    // Populate network throughput and queues
    server.ingress_packets_total = 450_000;
    server.ingress_bytes_total = 18_900_000;
    server.egress_packets_total = 980_000;
    server.egress_bytes_total = 41_200_000;
    server.queue_ingress_depth = 12;
    server.queue_egress_depth = 48;
    server.wal_queue_depth = 4;
    server.active_connections = 2_000;
    server.ingress_stale_dropped = 5;
    server.egress_unreliable_dropped = 18;

    let zones = [
        ZoneMetrics {
            zone_id: 101,
            active_entities: 1_200,
            active_observers: 450,
            hotspot_density_index: 85,
            seam_migrations_total: 128,
        },
        ZoneMetrics {
            zone_id: 102,
            active_entities: 800,
            active_observers: 300,
            hotspot_density_index: 42,
            seam_migrations_total: 94,
        },
    ];

    let rendered = PrometheusExporter::render_to_string(&server, &zones);

    // Verify Prometheus format compliance
    assert!(rendered.contains("# HELP eidolon_tick_duration_micros Authoritative simulation tick duration in microseconds"));
    assert!(rendered.contains("# TYPE eidolon_tick_duration_micros summary"));
    assert!(rendered.contains("eidolon_tick_duration_micros{quantile=\"0.5\"} 5000"));
    assert!(rendered.contains("eidolon_tick_duration_micros_count 500"));

    // Verify Phase Latencies
    assert!(rendered.contains("eidolon_phase_duration_micros{phase=\"ingress\"} 1150"));
    assert!(rendered.contains("eidolon_phase_duration_micros{phase=\"sim\"} 850"));
    assert!(rendered.contains("eidolon_phase_duration_micros{phase=\"spatial\"} 2100"));
    assert!(rendered.contains("eidolon_phase_duration_micros{phase=\"egress\"} 620"));

    // Verify Network and Queues
    assert!(rendered.contains("eidolon_network_packets_total{direction=\"ingress\"} 450000"));
    assert!(rendered.contains("eidolon_network_packets_total{direction=\"egress\"} 980000"));
    assert!(rendered.contains("eidolon_queue_depth{queue=\"ingress\"} 12"));
    assert!(rendered.contains("eidolon_queue_depth{queue=\"egress\"} 48"));
    assert!(rendered.contains("eidolon_queue_depth{queue=\"wal\"} 4"));
    assert!(rendered.contains("eidolon_active_connections 2000"));

    // Verify Zone breakdown
    assert!(rendered.contains("eidolon_zone_entities{zone_id=\"101\"} 1200"));
    assert!(rendered.contains("eidolon_zone_entities{zone_id=\"102\"} 800"));
    assert!(rendered.contains("eidolon_zone_hotspot_density{zone_id=\"101\"} 85"));
    assert!(rendered.contains("eidolon_zone_hotspot_density{zone_id=\"102\"} 42"));
    assert!(rendered.contains("eidolon_zone_seam_migrations_total{zone_id=\"101\"} 128"));
    assert!(rendered.contains("eidolon_zone_seam_migrations_total{zone_id=\"102\"} 94"));
}

#[test]
fn test_milestone_11_1_bounded_label_cardinality_invariant() {
    let server = ServerMetrics {
        active_connections: 10_000,
        ..Default::default()
    };

    let zones = [ZoneMetrics {
        zone_id: 1,
        active_entities: 5_000,
        active_observers: 2_500,
        hotspot_density_index: 120,
        seam_migrations_total: 650,
    }];

    let rendered = PrometheusExporter::render_to_string(&server, &zones);

    // Assert that strictly zero high-cardinality per-player or per-session labels exist
    assert!(!rendered.contains("player_id"));
    assert!(!rendered.contains("session_id"));
    assert!(!rendered.contains("account_id"));
    assert!(!rendered.contains("character_id"));
    assert!(!rendered.contains("peer_addr"));
    assert!(!rendered.contains("client_ip"));
    assert!(!rendered.contains("entity_id"));
}
