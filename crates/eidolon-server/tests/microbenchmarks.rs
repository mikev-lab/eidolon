//! High-precision dependency-free microbenchmark suite for the eidolon MMO engine.
//!
//! Validates nanosecond-level performance across core math, coordinate quantization,
//! bitstream packing, spatial queries, and queue throughput with zero external dependencies.

use std::hint::black_box;
use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::kinematics::{
    extrapolate, should_dispatch_update, DeadReckoningConfig, KinematicState, FLAG_WALKING,
};
use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw};
use eidolon_net::bitstream::{BitReader, BitWriter};
use eidolon_server::queue::{NetworkPacket, SpscPacketQueue};
use eidolon_spatial::grid::SpatialHashGrid;

fn run_bench<F: FnMut()>(name: &str, iterations: u64, mut op: F) -> f64 {
    // Warmup cycle (10% of iterations)
    let warmup_iters = (iterations / 10).max(50);
    for _ in 0..warmup_iters {
        op();
    }

    let start = Instant::now();
    for _ in 0..iterations {
        op();
    }
    let elapsed = start.elapsed();
    let total_nanos = elapsed.as_nanos();
    let nanos_per_op = (total_nanos as f64) / (iterations as f64);
    let ops_per_sec = (iterations as f64) / elapsed.as_secs_f64();

    println!(
        "{:<48} | {:>8} ops | {:>9.2} ns/op | {:>14.0} ops/sec",
        name, iterations, nanos_per_op, ops_per_sec
    );

    nanos_per_op
}

#[test]
fn test_engine_microbenchmarks() {
    println!("\n======================================================================================================");
    println!("                               eidolon High-Precision Microbenchmarks                                 ");
    println!("======================================================================================================");

    // Profile-aware latency thresholds:
    // - Release mode (--release): enforces strict sub-microsecond production budgets
    // - Debug mode: accounts for unoptimized stack frames and cloud CI virtual machine jitter
    let (
        max_mul,
        max_sqrt,
        max_clamp,
        max_dist,
        max_batch_dist,
        max_quant,
        max_pack,
        max_unpack,
        max_bitwriter,
        max_bitreader,
        max_grid_scalar,
        max_grid_batched,
        max_dr,
        max_div,
        max_queue,
    ) = if cfg!(debug_assertions) {
        (
            200.0, 2500.0, 150.0, 200.0, 400.0, 350.0, 250.0, 250.0, 2000.0, 2000.0, 100_000.0,
            100_000.0, 300.0, 250.0, 500.0,
        )
    } else {
        (
            3.0, 100.0, 2.0, 5.0, 15.0, 3.0, 2.0, 2.0, 50.0, 25.0, 6_000.0, 6_000.0, 15.0, 10.0,
            70.0,
        )
    };

    // 1. Fixed64 Math Benchmarks
    let a = Fixed64::from_f64(123.456);
    let b = Fixed64::from_f64(78.901);
    let min_b = Fixed64::from_f64(10.0);
    let max_b = Fixed64::from_f64(100.0);

    let ns_mul = run_bench("Fixed64::saturating_mul", 500_000, || {
        black_box(black_box(a) * black_box(b));
    });
    assert!(ns_mul < max_mul, "Fixed64 multiply threshold exceeded");

    let ns_sqrt = run_bench("Fixed64::sqrt", 200_000, || {
        black_box(black_box(a).sqrt());
    });
    assert!(ns_sqrt < max_sqrt, "Fixed64 sqrt threshold exceeded");

    let ns_clamp = run_bench("Fixed64::clamp (branchless)", 500_000, || {
        black_box(black_box(a).clamp(black_box(min_b), black_box(max_b)));
    });
    assert!(ns_clamp < max_clamp, "Fixed64 clamp threshold exceeded");

    // 2. Vec3Fix Distance & 4-Wide Batch SIMD Benchmarks
    let p1 = Vec3Fix::from_f64(10.0, 5.0, 15.0);
    let p2 = Vec3Fix::from_f64(25.0, 12.0, 40.0);
    let origins = [
        Vec3Fix::from_f64(1.0, 2.0, 3.0),
        Vec3Fix::from_f64(4.0, 5.0, 6.0),
        Vec3Fix::from_f64(7.0, 8.0, 9.0),
        Vec3Fix::from_f64(10.0, 11.0, 12.0),
    ];

    let ns_dist = run_bench("Vec3Fix::distance_squared (scalar)", 500_000, || {
        black_box(black_box(p1).distance_squared(black_box(p2)));
    });
    assert!(ns_dist < max_dist, "Scalar distance threshold exceeded");

    let ns_batch_dist = run_bench(
        "Vec3Fix::batch_distance_squared_4x (4-wide SIMD)",
        200_000,
        || {
            black_box(Vec3Fix::batch_distance_squared_4x(
                black_box(origins),
                black_box(p1),
            ));
        },
    );
    assert!(
        ns_batch_dist < max_batch_dist,
        "4-wide batch distance threshold exceeded"
    );

    // 3. Coordinate Quantization & 7-Byte Bitpacking
    let coord = QuantizedCellCoord::new(12345, 2048, 54321);
    let yaw = QuantizedYaw::from_degrees(180.0);
    let flags = 0x05;

    let ns_quant = run_bench("QuantizedCellCoord::quantize (branchless)", 500_000, || {
        black_box(QuantizedCellCoord::quantize(
            black_box(a),
            black_box(b),
            black_box(a),
        ));
    });
    assert!(ns_quant < max_quant, "Quantize threshold exceeded");

    let ns_pack = run_bench(
        "QuantizedCellCoord::pack_with_yaw_and_flags (7B)",
        500_000,
        || {
            black_box(coord.pack_with_yaw_and_flags(black_box(yaw), black_box(flags)));
        },
    );
    assert!(ns_pack < max_pack, "Bitpacking 7-byte threshold exceeded");

    let packed_bytes = coord.pack_with_yaw_and_flags(yaw, flags);
    let ns_unpack = run_bench(
        "QuantizedCellCoord::unpack_with_yaw_and_flags",
        500_000,
        || {
            black_box(QuantizedCellCoord::unpack_with_yaw_and_flags(black_box(
                packed_bytes,
            )));
        },
    );
    assert!(
        ns_unpack < max_unpack,
        "Unpacking 7-byte threshold exceeded"
    );

    // 4. Bitstream Writer & Reader Benchmarks
    let mut bit_buf = [0u8; 64];
    let ns_bitwriter = run_bench("BitWriter::write_bits (arbitrary width)", 200_000, || {
        let mut writer = BitWriter::new(&mut bit_buf);
        writer.write_bits(1, 1).unwrap();
        writer.write_bits(42, 7).unwrap();
        writer.write_bits(0xABCD, 16).unwrap();
        writer.write_u32(100_000).unwrap();
        black_box(writer.as_bytes());
    });
    assert!(ns_bitwriter < max_bitwriter, "BitWriter threshold exceeded");

    let ns_bitreader = run_bench("BitReader::read_bits", 200_000, || {
        let mut reader = BitReader::new(&bit_buf);
        let b1 = reader.read_bits(1).unwrap();
        let b2 = reader.read_bits(7).unwrap();
        let b3 = reader.read_bits(16).unwrap();
        let b4 = reader.read_u32().unwrap();
        black_box((b1, b2, b3, b4));
    });
    assert!(ns_bitreader < max_bitreader, "BitReader threshold exceeded");

    // 5. Spatial Hash Grid Queries (Scalar vs Batched)
    let mut grid = SpatialHashGrid::with_capacity(1000, 128);
    for i in 1..=500 {
        let pos = Vec3Fix::from_f64((i as f64) * 0.5, 0.0, (i as f64) * 0.5);
        grid.insert(i, pos).unwrap();
    }
    let query_center = Vec3Fix::from_f64(50.0, 0.0, 50.0);
    let radius_sq = Fixed64::from_i32(900); // 30m radius
    let mut out_buf = [0u32; 128];

    let ns_grid_scalar = run_bench("SpatialHashGrid::query_radius_squared", 100_000, || {
        black_box(grid.query_radius_squared(
            black_box(query_center),
            black_box(radius_sq),
            black_box(&mut out_buf),
        ));
    });
    assert!(
        ns_grid_scalar < max_grid_scalar,
        "Spatial grid query threshold exceeded"
    );

    let ns_grid_batched = run_bench(
        "SpatialHashGrid::query_radius_squared_batched",
        100_000,
        || {
            black_box(grid.query_radius_squared_batched(
                black_box(query_center),
                black_box(radius_sq),
                black_box(&mut out_buf),
            ));
        },
    );
    assert!(
        ns_grid_batched < max_grid_batched,
        "Batched spatial query threshold exceeded"
    );

    // 6. Dead Reckoning Extrapolation & Divergence
    let k_state = KinematicState::with_velocity(
        Vec3Fix::ZERO,
        Vec3Fix::from_f64(5.0, 0.0, 0.0),
        QuantizedYaw::NORTH,
        FLAG_WALKING,
    );
    let dt = Fixed64::from_f64(0.05);
    let dr_config = DeadReckoningConfig::default();

    let ns_dr = run_bench("kinematics::extrapolate", 500_000, || {
        black_box(extrapolate(black_box(&k_state), 1, black_box(dt)));
    });
    assert!(ns_dr < max_dr, "Extrapolate threshold exceeded");

    let ns_div = run_bench("kinematics::should_dispatch_update", 500_000, || {
        black_box(should_dispatch_update(
            black_box(&k_state),
            black_box(&k_state),
            10,
            black_box(&dr_config),
        ));
    });
    assert!(ns_div < max_div, "Divergence check threshold exceeded");

    // 7. SPSC Bounded Packet Queue Throughput
    let queue = SpscPacketQueue::<128>::new();
    let peer_addr = "127.0.0.1:8080".parse().unwrap();
    let packet = NetworkPacket::new(peer_addr, &[0x45, 0x49, 1, 2, 3]).unwrap();

    let ns_queue = run_bench("SpscPacketQueue::try_push + try_pop", 500_000, || {
        queue.try_push(black_box(packet));
        black_box(queue.try_pop());
    });
    assert!(
        ns_queue < max_queue,
        "SPSC queue roundtrip threshold exceeded"
    );

    println!("======================================================================================================\n");
}
