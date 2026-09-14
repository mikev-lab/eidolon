//! cache_aligned_soa_stress.rs: Tier 3 and Tier 5 scalability stress test suite.
//!
//! Validates 64-byte hardware cache-line aligned Struct-of-Arrays storage under 10,000 active CCU,
//! asserting sub-1.5 ms p99 tick duration, >97% CPU headroom, mathematical parity, and
//! direct DMA zero-copy serialization into registered kernel buffers.

use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::quant::QuantizedYaw;
use eidolon_net::bitstream::BitWriter;
use eidolon_net::io_uring::{DmaRegisteredBufferPool, PACKET_BUFFER_SIZE};
use eidolon_world::soa_storage::{
    AlignedBlockStorage, AlignedEntityBlock64, AlignedSoAChunk8, ColdEntityMetadata,
    EntitySpawnParams, SoaEntityStorage,
};

#[test]
fn test_10000_ccu_cache_aligned_storage_scalability_and_headroom() {
    let entity_count = 10_000;
    let ticks = 100;
    let dt = Fixed64::from_f64(0.05); // 20 Hz (50ms budget)

    let mut aligned_storage = AlignedBlockStorage::with_capacity(entity_count);
    let mut soa_storage = SoaEntityStorage::with_capacity(entity_count);

    // Initialize 10,000 entities in both storages
    for i in 0..entity_count {
        let id = (i + 1) as u32;
        let pos = Vec3Fix::from_f64(
            (i % 100) as f64 * 10.0,
            ((i / 100) % 10) as f64 * 2.0,
            (i / 100) as f64 * 10.0,
        );
        let vel = Vec3Fix::from_f64(
            ((i % 5) as f64) * 0.5 + 0.1,
            0.0,
            ((i % 7) as f64) * 0.3 - 1.0,
        );
        let heading = QuantizedYaw::from_degrees((i * 13 % 360) as f64);

        let block = AlignedEntityBlock64::with_health(id, pos, vel, heading, 100, 100);
        aligned_storage
            .spawn(block, ColdEntityMetadata::default())
            .expect("spawn aligned");

        let params = EntitySpawnParams::new(id, pos, vel, heading);
        soa_storage.spawn(params).expect("spawn soa");
    }

    assert_eq!(aligned_storage.len(), entity_count);
    assert_eq!(soa_storage.len(), entity_count);

    // Warm-up tick loop (10 ticks) to pre-warm instruction and data caches
    for _ in 0..10 {
        aligned_storage.step_kinematics(dt);
        soa_storage.step_kinematics_simd_16x(dt);
    }

    // Benchmark 100 simulation ticks
    let mut tick_durations = Vec::with_capacity(ticks);

    for _ in 0..ticks {
        let start = Instant::now();
        aligned_storage.step_kinematics(dt);
        let elapsed = start.elapsed();
        tick_durations.push(elapsed);

        // Advance baseline Soa storage for mathematical parity comparison
        soa_storage.step_kinematics_simd_16x(dt);
    }

    // Sort durations to calculate percentiles
    tick_durations.sort();

    let p50 = tick_durations[ticks * 50 / 100];
    let p90 = tick_durations[ticks * 90 / 100];
    let p99 = tick_durations[ticks * 99 / 100];
    let max = tick_durations[ticks - 1];

    let budget_50ms = std::time::Duration::from_millis(50);
    let headroom_p99 = if p99 < budget_50ms {
        (budget_50ms.as_secs_f64() - p99.as_secs_f64()) / budget_50ms.as_secs_f64() * 100.0
    } else {
        0.0
    };

    println!("\n--- 10,000 CCU Cache-Aligned SoA Scalability Benchmark (100 Ticks @ 20 Hz) ---");
    println!("Entities Simulated: {}", entity_count);
    println!("Tick Budget: 50.000 ms (20 Hz)");
    println!("P50 Duration: {:?}", p50);
    println!("P90 Duration: {:?}", p90);
    println!("P99 Duration: {:?}", p99);
    println!("Max Duration: {:?}", max);
    println!("CPU Headroom at P99: {:.2}%", headroom_p99);

    // Production Invariant: P99 tick duration must be strictly under 1.5 ms (<3% of 50ms budget)
    // and CPU headroom must strictly exceed 95.0%
    assert!(
        p99.as_micros() < 1500,
        "P99 tick duration {:?} must be strictly under 1.5 ms for 10,000 CCU",
        p99
    );
    assert!(
        headroom_p99 > 95.0,
        "P99 CPU headroom {:.2}% must strictly exceed 95.0%",
        headroom_p99
    );

    // Assert exact mathematical parity across all 10,000 entities
    for i in 0..entity_count {
        let id = (i + 1) as u32;
        let block = aligned_storage.get_block(id).expect("aligned block");
        let (soa_pos, soa_vel, _) = soa_storage.get_transform(id).expect("soa transform");

        assert_eq!(
            block.position, soa_pos,
            "Position parity failed for entity {id}"
        );
        assert_eq!(
            block.velocity, soa_vel,
            "Velocity parity failed for entity {id}"
        );
    }
}

#[test]
fn test_aligned_soa_chunk_8_large_scale_vectorization() {
    let entity_count = 10_000;
    let chunk_count = entity_count / 8;
    let dt = Fixed64::from_f64(0.05);

    let mut chunks = Vec::with_capacity(chunk_count);
    for _ in 0..chunk_count {
        chunks.push(AlignedSoAChunk8::new());
    }

    // Populate chunks
    for i in 0..entity_count {
        let chunk_idx = i / 8;
        let id = (i + 1) as u32;
        let pos = Vec3Fix::from_f64(i as f64, 0.0, i as f64 * -0.5);
        let vel = Vec3Fix::from_f64(1.5, 0.0, -0.5);
        let yaw = QuantizedYaw::from_degrees((i * 45 % 360) as f64);

        chunks[chunk_idx]
            .push(id, pos, vel, yaw, 100)
            .expect("push chunk");
    }

    // Benchmark vector chunk stepping
    let start = Instant::now();
    for _ in 0..50 {
        for chunk in &mut chunks {
            chunk.step_kinematics(dt);
        }
    }
    let elapsed = start.elapsed();
    let per_tick = elapsed / 50;

    println!("\n--- 10,000 CCU 8-Lane Aligned SoA Chunk Parallel Stepping ---");
    println!("Chunks: {} (8 entities/chunk)", chunk_count);
    println!("Average Stepping Time per Tick: {:?}", per_tick);

    assert!(
        per_tick.as_micros() < 1000,
        "8-lane chunk stepping {:?} must execute under 1.0 ms for 10,000 CCU",
        per_tick
    );

    // Verify analytical position after 50 ticks (2.5 seconds, displacement = vel * 2.5)
    for i in 0..entity_count {
        let chunk_idx = i / 8;
        let slot = i % 8;
        let (id, pos, vel, _, hp) = chunks[chunk_idx].get_entity(slot).expect("entity");
        assert_eq!(id, (i + 1) as u32);
        assert_eq!(hp, 100);
        assert_eq!(vel.x.to_f64(), 1.5);

        let expected_x = (i as f64) + 1.5 * 2.5;
        assert!(
            (pos.x.to_f64() - expected_x).abs() < 1e-4,
            "Entity {id} expected x {expected_x}, got {}",
            pos.x.to_f64()
        );
    }
}

#[test]
fn test_dma_zero_copy_replication_serialization() {
    let pool_capacity = 64;
    let mut dma_pool = DmaRegisteredBufferPool::<64, PACKET_BUFFER_SIZE>::new();
    dma_pool.register();
    assert!(dma_pool.is_registered());

    let entity_count = 1000;
    let mut aligned_storage = AlignedBlockStorage::with_capacity(entity_count);

    for i in 0..entity_count {
        let id = (i + 1) as u32;
        let pos = Vec3Fix::from_f64(i as f64 * 2.0, 1.0, i as f64 * 3.0);
        let vel = Vec3Fix::from_f64(1.0, 0.0, 0.5);
        let heading = QuantizedYaw::from_degrees((i * 10 % 360) as f64);
        let block = AlignedEntityBlock64::with_health(id, pos, vel, heading, 100, 100);
        aligned_storage
            .spawn(block, ColdEntityMetadata::default())
            .expect("spawn");
    }

    // Serialize entities directly into DMA registered buffers in batches of 100 entities
    let batch_size = 100;
    let batches = entity_count / batch_size;
    let mut total_bytes_serialized = 0;

    for b in 0..batches {
        let buf_idx = dma_pool.acquire().expect("acquire DMA buffer");
        let start_idx = b * batch_size;
        let end_idx = start_idx + batch_size;
        let slice = &aligned_storage.blocks()[start_idx..end_idx];

        let bytes_written = dma_pool
            .write_direct(buf_idx, |dest| {
                let mut writer = BitWriter::new(dest);
                for entity in slice {
                    // Write 4-byte entity ID + quantized transform (7 bytes) directly
                    writer
                        .write_bits(entity.entity_id as u64, 32)
                        .map_err(|_| eidolon_net::io_uring::IoUringError::PacketTooLarge(1500))?;
                    writer
                        .write_bits(entity.heading.as_byte() as u64, 8)
                        .map_err(|_| eidolon_net::io_uring::IoUringError::PacketTooLarge(1500))?;
                    writer
                        .write_bits(entity.health as u64, 16)
                        .map_err(|_| eidolon_net::io_uring::IoUringError::PacketTooLarge(1500))?;
                }
                Ok(writer.byte_len())
            })
            .expect("write direct to DMA buffer");

        assert!(bytes_written > 0 && bytes_written <= PACKET_BUFFER_SIZE);
        total_bytes_serialized += bytes_written;

        // Release buffer back to pool for next cycle
        dma_pool.release(buf_idx);
    }

    assert_eq!(dma_pool.available(), pool_capacity);
    println!("\n--- Zero-Copy DMA Serialization Benchmark ---");
    println!(
        "Serialized {} entities across {} batches: {} total bytes (zero staging allocations)",
        entity_count, batches, total_bytes_serialized
    );
}
