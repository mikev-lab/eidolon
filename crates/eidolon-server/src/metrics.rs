//! Bounded-cardinality Prometheus metrics, latency histograms, and phase telemetry.
//!
//! Exposes server-wide and zone-level operational metrics using standard Prometheus text
//! exposition format with strictly zero per-player metric labels to protect monitoring
//! clusters from out-of-memory cardinality explosions.

use std::fmt::Write;

/// Default bucket thresholds in microseconds for the simulation tick duration histogram.
pub const DEFAULT_TICK_BUCKETS_MICROS: [u64; 16] = [
    500, 1_000, 2_000, 5_000, 10_000, 20_000, 30_000, 35_000, 40_000, 45_000, 48_000, 50_000,
    60_000, 75_000, 100_000, 200_000,
];

/// Fixed-memory logarithmic latency histogram computing percentiles with zero heap allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencyHistogram<const BUCKETS: usize> {
    bucket_bounds: [u64; BUCKETS],
    bucket_counts: [u64; BUCKETS],
    overflow_count: u64,
    total_count: u64,
    total_sum_micros: u64,
    min_micros: u64,
    max_micros: u64,
}

impl<const BUCKETS: usize> LatencyHistogram<BUCKETS> {
    /// Creates a new latency histogram with custom upper bounds.
    pub const fn new(bucket_bounds: [u64; BUCKETS]) -> Self {
        Self {
            bucket_bounds,
            bucket_counts: [0; BUCKETS],
            overflow_count: 0,
            total_count: 0,
            total_sum_micros: 0,
            min_micros: u64::MAX,
            max_micros: 0,
        }
    }

    /// Records a single latency observation in microseconds.
    pub fn record(&mut self, micros: u64) {
        self.total_count += 1;
        self.total_sum_micros = self.total_sum_micros.saturating_add(micros);

        if micros < self.min_micros {
            self.min_micros = micros;
        }
        if micros > self.max_micros {
            self.max_micros = micros;
        }

        let mut placed = false;
        for i in 0..BUCKETS {
            if micros <= self.bucket_bounds[i] {
                self.bucket_counts[i] += 1;
                placed = true;
                break;
            }
        }

        if !placed {
            self.overflow_count += 1;
        }
    }

    /// Returns the total number of recorded observations.
    #[inline]
    pub const fn count(&self) -> u64 {
        self.total_count
    }

    /// Returns the sum of all recorded observations in microseconds.
    #[inline]
    pub const fn sum_micros(&self) -> u64 {
        self.total_sum_micros
    }

    /// Returns the minimum observed latency in microseconds.
    #[inline]
    pub const fn min_micros(&self) -> u64 {
        if self.total_count == 0 {
            0
        } else {
            self.min_micros
        }
    }

    /// Returns the maximum observed latency in microseconds.
    #[inline]
    pub const fn max_micros(&self) -> u64 {
        self.max_micros
    }

    /// Calculates a target percentile (e.g. 0.50, 0.95, 0.99, 0.999) using bucket boundaries.
    pub fn percentile(&self, p: f64) -> u64 {
        if self.total_count == 0 {
            return 0;
        }

        let rank = ((self.total_count as f64) * p.clamp(0.0, 1.0)).ceil() as u64;
        let mut running_sum = 0;

        for i in 0..BUCKETS {
            running_sum += self.bucket_counts[i];
            if running_sum >= rank {
                return self.bucket_bounds[i];
            }
        }

        self.max_micros
    }
}

impl Default for LatencyHistogram<16> {
    fn default() -> Self {
        Self::new(DEFAULT_TICK_BUCKETS_MICROS)
    }
}

/// Server-wide metrics aggregating tick latency, phase timings, throughput, and queue states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ServerMetrics {
    /// Latency histogram tracking tick duration percentiles.
    pub tick_histogram: LatencyHistogram<16>,
    /// Microsecond execution duration for phase 1 (ingress drain).
    pub phase_ingress_micros: u64,
    /// Microsecond execution duration for phase 2 (simulation world update).
    pub phase_sim_micros: u64,
    /// Microsecond execution duration for phase 3 (spatial partitioning and AoI).
    pub phase_spatial_micros: u64,
    /// Microsecond execution duration for phase 4 (egress flush).
    pub phase_egress_micros: u64,
    /// Total ingress packets received.
    pub ingress_packets_total: u64,
    /// Total ingress bytes received.
    pub ingress_bytes_total: u64,
    /// Total egress packets dispatched.
    pub egress_packets_total: u64,
    /// Total egress bytes dispatched.
    pub egress_bytes_total: u64,
    /// Current depth of the network ingress queue.
    pub queue_ingress_depth: usize,
    /// Current depth of the network egress queue.
    pub queue_egress_depth: usize,
    /// Current depth of the write-ahead journal ring buffer.
    pub wal_queue_depth: usize,
    /// Current count of active authenticated client sessions.
    pub active_connections: usize,
    /// Stale ingress packets dropped under saturation.
    pub ingress_stale_dropped: u64,
    /// Oldest egress transforms dropped under saturation.
    pub egress_unreliable_dropped: u64,
}

/// Zone-level metrics aggregating entity density, observer interest, and seam migrations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ZoneMetrics {
    /// Numeric zone identifier.
    pub zone_id: u16,
    /// Number of active entities currently situated in this zone.
    pub active_entities: usize,
    /// Number of connected observers tracking this zone.
    pub active_observers: usize,
    /// Maximum number of entities clustered into any single 64m spatial cell.
    pub hotspot_density_index: usize,
    /// Total entity migrations across boundary seams originating from this zone.
    pub seam_migrations_total: u64,
}

/// Native text exporter rendering Prometheus compliant metric exposition.
#[derive(Debug, Default)]
pub struct PrometheusExporter;

impl PrometheusExporter {
    /// Formats all server and zone metrics into standard Prometheus text format.
    ///
    /// Guaranteed invariant: strictly zero per-player metric labels are emitted.
    pub fn render_to_string(server: &ServerMetrics, zones: &[ZoneMetrics]) -> String {
        let mut out = String::with_capacity(4096);
        Self::format_prometheus(server, zones, &mut out);
        out
    }

    /// Formats metrics directly into an existing string buffer.
    pub fn format_prometheus(server: &ServerMetrics, zones: &[ZoneMetrics], out: &mut String) {
        // 1. Tick duration summary
        let _ = writeln!(
            out,
            "# HELP eidolon_tick_duration_micros Authoritative simulation tick duration in microseconds\n# TYPE eidolon_tick_duration_micros summary"
        );
        let _ = writeln!(
            out,
            "eidolon_tick_duration_micros{{quantile=\"0.5\"}} {}",
            server.tick_histogram.percentile(0.50)
        );
        let _ = writeln!(
            out,
            "eidolon_tick_duration_micros{{quantile=\"0.95\"}} {}",
            server.tick_histogram.percentile(0.95)
        );
        let _ = writeln!(
            out,
            "eidolon_tick_duration_micros{{quantile=\"0.99\"}} {}",
            server.tick_histogram.percentile(0.99)
        );
        let _ = writeln!(
            out,
            "eidolon_tick_duration_micros{{quantile=\"0.999\"}} {}",
            server.tick_histogram.percentile(0.999)
        );
        let _ = writeln!(
            out,
            "eidolon_tick_duration_micros_count {}",
            server.tick_histogram.count()
        );
        let _ = writeln!(
            out,
            "eidolon_tick_duration_micros_sum {}",
            server.tick_histogram.sum_micros()
        );

        // 2. Phase execution latencies
        let _ = writeln!(
            out,
            "# HELP eidolon_phase_duration_micros Microsecond execution latency per tick phase\n# TYPE eidolon_phase_duration_micros gauge"
        );
        let _ = writeln!(
            out,
            "eidolon_phase_duration_micros{{phase=\"ingress\"}} {}",
            server.phase_ingress_micros
        );
        let _ = writeln!(
            out,
            "eidolon_phase_duration_micros{{phase=\"sim\"}} {}",
            server.phase_sim_micros
        );
        let _ = writeln!(
            out,
            "eidolon_phase_duration_micros{{phase=\"spatial\"}} {}",
            server.phase_spatial_micros
        );
        let _ = writeln!(
            out,
            "eidolon_phase_duration_micros{{phase=\"egress\"}} {}",
            server.phase_egress_micros
        );

        // 3. Network traffic throughput
        let _ = writeln!(
            out,
            "# HELP eidolon_network_packets_total Total UDP packets processed\n# TYPE eidolon_network_packets_total counter"
        );
        let _ = writeln!(
            out,
            "eidolon_network_packets_total{{direction=\"ingress\"}} {}",
            server.ingress_packets_total
        );
        let _ = writeln!(
            out,
            "eidolon_network_packets_total{{direction=\"egress\"}} {}",
            server.egress_packets_total
        );

        let _ = writeln!(
            out,
            "# HELP eidolon_network_bytes_total Total UDP payload bytes processed\n# TYPE eidolon_network_bytes_total counter"
        );
        let _ = writeln!(
            out,
            "eidolon_network_bytes_total{{direction=\"ingress\"}} {}",
            server.ingress_bytes_total
        );
        let _ = writeln!(
            out,
            "eidolon_network_bytes_total{{direction=\"egress\"}} {}",
            server.egress_bytes_total
        );

        // 4. Queue depths
        let _ = writeln!(
            out,
            "# HELP eidolon_queue_depth Current item depth of pipeline ring buffers\n# TYPE eidolon_queue_depth gauge"
        );
        let _ = writeln!(
            out,
            "eidolon_queue_depth{{queue=\"ingress\"}} {}",
            server.queue_ingress_depth
        );
        let _ = writeln!(
            out,
            "eidolon_queue_depth{{queue=\"egress\"}} {}",
            server.queue_egress_depth
        );
        let _ = writeln!(
            out,
            "eidolon_queue_depth{{queue=\"wal\"}} {}",
            server.wal_queue_depth
        );

        // 5. Active connections
        let _ = writeln!(
            out,
            "# HELP eidolon_active_connections Current active authenticated client connections\n# TYPE eidolon_active_connections gauge"
        );
        let _ = writeln!(
            out,
            "eidolon_active_connections {}",
            server.active_connections
        );

        // 6. Drops
        let _ = writeln!(
            out,
            "# HELP eidolon_dropped_packets_total Total packets dropped under backpressure\n# TYPE eidolon_dropped_packets_total counter"
        );
        let _ = writeln!(
            out,
            "eidolon_dropped_packets_total{{reason=\"ingress_stale\"}} {}",
            server.ingress_stale_dropped
        );
        let _ = writeln!(
            out,
            "eidolon_dropped_packets_total{{reason=\"egress_unreliable\"}} {}",
            server.egress_unreliable_dropped
        );

        // 7. Zone-level metrics (bounded by zone_id)
        if !zones.is_empty() {
            let _ = writeln!(
                out,
                "# HELP eidolon_zone_entities Current active entities in zone\n# TYPE eidolon_zone_entities gauge"
            );
            for zone in zones {
                let _ = writeln!(
                    out,
                    "eidolon_zone_entities{{zone_id=\"{}\"}} {}",
                    zone.zone_id, zone.active_entities
                );
            }

            let _ = writeln!(
                out,
                "# HELP eidolon_zone_hotspot_density Maximum entities in a single 64m cell\n# TYPE eidolon_zone_hotspot_density gauge"
            );
            for zone in zones {
                let _ = writeln!(
                    out,
                    "eidolon_zone_hotspot_density{{zone_id=\"{}\"}} {}",
                    zone.zone_id, zone.hotspot_density_index
                );
            }

            let _ = writeln!(
                out,
                "# HELP eidolon_zone_seam_migrations_total Cumulative entity migrations across seam\n# TYPE eidolon_zone_seam_migrations_total counter"
            );
            for zone in zones {
                let _ = writeln!(
                    out,
                    "eidolon_zone_seam_migrations_total{{zone_id=\"{}\"}} {}",
                    zone.zone_id, zone.seam_migrations_total
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latency_histogram_percentile_calculation() {
        let mut hist: LatencyHistogram<16> = LatencyHistogram::default();

        // Record 100 samples of 2500 µs (bucket 5000)
        for _ in 0..90 {
            hist.record(2500);
        }
        // Record 5 samples of 32000 µs (bucket 35000)
        for _ in 0..5 {
            hist.record(32000);
        }
        // Record 4 samples of 42000 µs (bucket 45000)
        for _ in 0..4 {
            hist.record(42000);
        }
        // Record 1 sample of 49000 µs (bucket 50000)
        hist.record(49000);

        assert_eq!(hist.count(), 100);
        assert_eq!(hist.min_micros(), 2500);
        assert_eq!(hist.max_micros(), 49000);

        // p50 is within 2500µs (bucket bound 5000)
        assert_eq!(hist.percentile(0.50), 5000);
        // p95 is at 32000µs (bucket bound 35000)
        assert_eq!(hist.percentile(0.95), 35000);
        // p99 is at 42000µs (bucket bound 45000)
        assert_eq!(hist.percentile(0.99), 45000);
        // p99.9 hits max bucket
        assert_eq!(hist.percentile(0.999), 50000);
    }

    #[test]
    fn test_prometheus_exposition_format_and_bounded_cardinality() {
        let mut server = ServerMetrics::default();
        server.tick_histogram.record(3000);
        server.tick_histogram.record(4500);
        server.phase_ingress_micros = 1200;
        server.phase_sim_micros = 50;
        server.phase_spatial_micros = 2100;
        server.phase_egress_micros = 400;
        server.active_connections = 150;

        let zones = [
            ZoneMetrics {
                zone_id: 1,
                active_entities: 500,
                active_observers: 150,
                hotspot_density_index: 45,
                seam_migrations_total: 12,
            },
            ZoneMetrics {
                zone_id: 2,
                active_entities: 300,
                active_observers: 80,
                hotspot_density_index: 20,
                seam_migrations_total: 8,
            },
        ];

        let output = PrometheusExporter::render_to_string(&server, &zones);

        // Assert standard Prometheus header directives
        assert!(output.contains("# HELP eidolon_tick_duration_micros"));
        assert!(output.contains("# TYPE eidolon_tick_duration_micros summary"));
        assert!(output.contains("eidolon_tick_duration_micros{quantile=\"0.5\"}"));
        assert!(output.contains("eidolon_phase_duration_micros{phase=\"ingress\"} 1200"));
        assert!(output.contains("eidolon_active_connections 150"));
        assert!(output.contains("eidolon_zone_entities{zone_id=\"1\"} 500"));
        assert!(output.contains("eidolon_zone_entities{zone_id=\"2\"} 300"));

        // Strictly verify zero per-player metric labels
        assert!(!output.contains("player_id"));
        assert!(!output.contains("session_id"));
        assert!(!output.contains("account_id"));
    }
}
