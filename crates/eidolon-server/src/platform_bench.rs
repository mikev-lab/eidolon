//! Platform Microbenchmark Suite and Cross-Architecture Hardware Characterization.
//!
//! Measures micro-architectural throughput on the host processor (Apple Silicon M-series or x86-64 server),
//! characterizing coordinate quantization, spatial hash indexing, and bitstream serialization speed.

use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::quant::QuantizedCellCoord;
use eidolon_net::bitstream::{BitReader, BitWriter};
use eidolon_spatial::grid::SpatialHashGrid;

/// Results of a single platform microbenchmark suite.
#[derive(Debug, Clone, Copy)]
pub struct BenchmarkMetric {
    /// Name of the evaluated operation.
    pub name: &'static str,
    /// Total iterations executed.
    pub iterations: u64,
    /// Total duration elapsed in microseconds.
    pub elapsed_micros: u64,
    /// Average duration per operation in nanoseconds.
    pub ns_per_op: f64,
    /// Throughput in millions of operations per second (Mops/s).
    pub mops_per_sec: f64,
}

/// Comprehensive hardware characterization report.
#[derive(Debug, Clone)]
pub struct PlatformBenchmarkReport {
    /// Host operating system and target architecture description.
    pub target_arch: &'static str,
    /// Coordinate quantization benchmark results.
    pub quantization_metric: BenchmarkMetric,
    /// Spatial hash grid indexing benchmark results.
    pub spatial_grid_metric: BenchmarkMetric,
    /// Register-width bitstream serialization benchmark results.
    pub bitstream_metric: BenchmarkMetric,
}

/// Automated runner executing hardware characterization suites.
pub struct PlatformBenchmarkRunner;

impl PlatformBenchmarkRunner {
    /// Executes the full benchmark battery and returns structured metrics.
    pub fn run_benchmark_suite(iterations: u64) -> PlatformBenchmarkReport {
        let quant = Self::benchmark_quantization(iterations);
        let spatial = Self::benchmark_spatial_grid(iterations.min(50_000));
        let bitstream = Self::benchmark_bitstream(iterations);

        let target_arch = if cfg!(target_arch = "aarch64") {
            "ARM64 (Apple Silicon / aarch64)"
        } else if cfg!(target_arch = "x86_64") {
            "x86-64 (Intel Xeon / AMD EPYC)"
        } else {
            "Generic Target Architecture"
        };

        PlatformBenchmarkReport {
            target_arch,
            quantization_metric: quant,
            spatial_grid_metric: spatial,
            bitstream_metric: bitstream,
        }
    }

    fn benchmark_quantization(iterations: u64) -> BenchmarkMetric {
        let pos = Vec3Fix::from_f64(12.345, 5.678, 43.210);

        let start = Instant::now();
        let mut sink = 0u64;

        for i in 0..iterations {
            let offset_pos = Vec3Fix::new(pos.x + Fixed64::from_raw(i as i64), pos.y, pos.z);
            let quantized = QuantizedCellCoord::quantize(offset_pos.x, offset_pos.y, offset_pos.z);
            sink ^= quantized.x as u64;
        }

        let elapsed = start.elapsed();
        let elapsed_micros = elapsed.as_micros() as u64;
        let total_nanos = elapsed.as_nanos() as f64;
        let ns_per_op = total_nanos / iterations as f64;
        let mops_per_sec = (iterations as f64) / (total_nanos / 1_000.0);

        // Prevent dead-code elimination
        let _ = sink;

        BenchmarkMetric {
            name: "Coordinate Quantization (QuantizedCellCoord::quantize)",
            iterations,
            elapsed_micros,
            ns_per_op,
            mops_per_sec,
        }
    }

    fn benchmark_spatial_grid(iterations: u64) -> BenchmarkMetric {
        let mut grid = SpatialHashGrid::with_capacity(600, 64);
        let mut query_results = [0u32; 64];

        // Seed 500 initial entities
        for id in 1..=500 {
            let pos = Vec3Fix::from_f64((id as f64) * 0.5, 0.0, (id as f64) * 0.5);
            let _ = grid.insert(id, pos);
        }

        let center = Vec3Fix::from_f64(50.0, 0.0, 50.0);
        let radius_sq = Fixed64::from_f64(625.0); // 25m radius squared

        let start = Instant::now();
        let mut total_found = 0usize;

        for _ in 0..iterations {
            let res = grid.query_radius_squared(center, radius_sq, &mut query_results);
            total_found += res.written;
        }

        let elapsed = start.elapsed();
        let elapsed_micros = elapsed.as_micros() as u64;
        let total_nanos = elapsed.as_nanos() as f64;
        let ns_per_op = total_nanos / iterations as f64;
        let mops_per_sec = (iterations as f64) / (total_nanos / 1_000.0);

        let _ = total_found;

        BenchmarkMetric {
            name: "Spatial Hash Grid Query (SpatialHashGrid::query_radius_squared)",
            iterations,
            elapsed_micros,
            ns_per_op,
            mops_per_sec,
        }
    }

    fn benchmark_bitstream(iterations: u64) -> BenchmarkMetric {
        let mut buffer = [0u8; 64];

        let start = Instant::now();
        let mut sink = 0u64;

        for _ in 0..iterations {
            let (byte_len, bits) = {
                let mut writer = BitWriter::new(&mut buffer);
                let _ = writer.write_bits(0xAB, 8);
                let _ = writer.write_bits(0x0F, 4);
                let _ = writer.write_bits(0x1234, 16);
                (writer.byte_len(), writer.bit_offset())
            };

            let mut reader = BitReader::new(&buffer[..byte_len]);
            let val1 = reader.read_bits(8).unwrap_or(0);
            let val2 = reader.read_bits(4).unwrap_or(0);
            let val3 = reader.read_bits(16).unwrap_or(0);

            sink ^= val1 ^ val2 ^ val3 ^ (bits as u64);
        }

        let elapsed = start.elapsed();
        let elapsed_micros = elapsed.as_micros() as u64;
        let total_nanos = elapsed.as_nanos() as f64;
        let ns_per_op = total_nanos / iterations as f64;
        let mops_per_sec = (iterations as f64) / (total_nanos / 1_000.0);

        let _ = sink;

        BenchmarkMetric {
            name: "Bitstream Serialization (BitWriter/BitReader Roundtrip)",
            iterations,
            elapsed_micros,
            ns_per_op,
            mops_per_sec,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_benchmark_suite_execution() {
        let report = PlatformBenchmarkRunner::run_benchmark_suite(10_000);

        assert!(!report.target_arch.is_empty());
        assert!(report.quantization_metric.ns_per_op > 0.0);
        assert!(report.spatial_grid_metric.ns_per_op > 0.0);
        assert!(report.bitstream_metric.ns_per_op > 0.0);

        // Nanoseconds per quantization must remain < 500 ns (even in unoptimized debug builds)
        assert!(
            report.quantization_metric.ns_per_op < 500.0,
            "Quantization ({:.2} ns/op) must be high-throughput",
            report.quantization_metric.ns_per_op
        );
    }
}
