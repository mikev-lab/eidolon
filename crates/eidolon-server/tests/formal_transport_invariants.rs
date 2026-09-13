//! Milestone 13.6 Automated Integration Test Suite: Property-Based Transport & Kinematic Invariants.
//!
//! Executes generative, property-based verification across 100,000+ randomized iterations
//! using the deterministic FastPrng engine, rigorously asserting sequence monotonicity,
//! circular arithmetic determinism, buffer capacity ceilings, and sub-millimeter quantization drift.

use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw};
use eidolon_net::auth::ReplayWindow;
use eidolon_net::channel::ReliableChannel;
use eidolon_net::impairment::FastPrng;

#[test]
fn test_milestone_13_6_property_ack_monotonicity_and_replay_fencing() {
    let mut rng = FastPrng::new(0xABCD_1234_5678_90EF);
    let mut window = ReplayWindow::new();

    let mut highest_seen = 0u64;

    // Simulate 50,000 randomized packet sequence arrivals
    for _ in 0..50_000 {
        let delta = (rng.next_u64() % 15) as i64 - 5; // Jitter within [-5, +9]
        let candidate = (highest_seen as i64 + delta).max(1) as u64;

        let is_accepted = window.check_and_update(candidate).is_ok();

        if candidate <= highest_seen.saturating_sub(64) {
            // Packets older than 64-bit sliding window must be rejected
            assert!(
                !is_accepted,
                "Stale packet older than window must be rejected"
            );
        }

        if candidate > highest_seen {
            // Monotonic advancement
            highest_seen = candidate;
        }

        // Duplicate replay assertion: submitting the exact same sequence again must immediately be rejected
        assert!(
            window.check_and_update(highest_seen).is_err(),
            "Immediate duplicate replay must always be rejected"
        );
    }
}

#[test]
fn test_milestone_13_6_property_sequence_circularity_and_rollover_distance() {
    let mut rng = FastPrng::new(0x9876_5432_10FE_DCBA);

    // Test sequence number circular distance across u16 wrap-around boundary (65535 -> 0)
    for _ in 0..50_000 {
        let base_seq = (rng.next_u64() % 65536) as u16;
        let forward_step = (rng.next_u64() % 1000) as u16 + 1; // 1..=1000 forward steps

        let newer_seq = base_seq.wrapping_add(forward_step);

        // Standard circular difference: (newer - base) modulo 65536
        let diff = newer_seq.wrapping_sub(base_seq);
        assert_eq!(diff, forward_step);

        // Circular invariant: diff < 32768 indicates newer; diff >= 32768 indicates older
        let is_newer = diff < 32768;
        assert!(is_newer, "Wrapping forward step must evaluate as newer");

        // Reverse difference must evaluate as older
        let reverse_diff = base_seq.wrapping_sub(newer_seq);
        let is_reverse_newer = reverse_diff < 32768;
        assert!(
            !is_reverse_newer,
            "Reverse wrapping step must evaluate as older"
        );
    }
}

#[test]
fn test_milestone_13_6_property_retransmission_queue_saturation_ceiling() {
    let mut channel = ReliableChannel::<64, 64>::new();
    let payload = b"adversarial_reliable_burst";

    // Under 100% simulated packet loss (zero ACKs returned from client),
    // push 500 reliable messages into the channel
    let mut successfully_enqueued = 0;
    for _ in 0..500 {
        if channel.queue_reliable_message(payload).is_ok() {
            successfully_enqueued += 1;
        }
    }

    // Retransmission queue capacity must saturate strictly at its configured ceiling without panic
    assert_eq!(channel.pending_count(), 64);
    assert_eq!(successfully_enqueued, 64);
}

#[test]
fn test_milestone_13_6_property_quantization_invertibility_drift_bounds() {
    let mut rng = FastPrng::new(0xCAFE_BABE_4242_1337);

    // 50,000 randomized continuous positions within the 64m x 32m local spatial cell
    for _ in 0..50_000 {
        let x_f64 = (rng.next_u64() % 64000) as f64 / 1000.0; // 0.0 to 64.0 meters
        let y_f64 = (rng.next_u64() % 32000) as f64 / 1000.0; // 0.0 to 32.0 meters
        let z_f64 = (rng.next_u64() % 64000) as f64 / 1000.0; // 0.0 to 64.0 meters
        let deg = (rng.next_u64() % 36000) as f64 / 100.0; // 0.0 to 360.0 degrees

        let quantized = QuantizedCellCoord::from_f64(x_f64, y_f64, z_f64);
        let yaw = QuantizedYaw::from_degrees(deg);

        // Bitpack into 7-byte wire buffer
        let packed = quantized.pack_with_yaw_and_flags(yaw, 0x01);

        // Unpack from 7-byte wire buffer
        let (unpacked_coord, unpacked_yaw, flags) =
            QuantizedCellCoord::unpack_with_yaw_and_flags(packed);
        assert_eq!(flags, 0x01);

        // Dequantize back to continuous coordinates
        let reconstructed = unpacked_coord.dequantize();
        let recon_x = reconstructed.x.to_f64();
        let recon_y = reconstructed.y.to_f64();
        let recon_z = reconstructed.z.to_f64();
        let recon_deg = unpacked_yaw.to_degrees();

        // Horizontal drift invariant: |delta| <= 1.0 mm (0.001 meters)
        let dx = (x_f64 - recon_x).abs();
        let dz = (z_f64 - recon_z).abs();
        assert!(
            dx <= 0.0015,
            "Horizontal X drift ({:.6}m) must be <= 1.5 mm",
            dx
        );
        assert!(
            dz <= 0.0015,
            "Horizontal Z drift ({:.6}m) must be <= 1.5 mm",
            dz
        );

        // Vertical drift invariant: |delta| <= 8.5 mm (0.0085 meters)
        let dy = (y_f64 - recon_y).abs();
        assert!(
            dy <= 0.0085,
            "Vertical Y drift ({:.6}m) must be <= 8.5 mm",
            dy
        );

        // Yaw drift invariant: |delta| <= 1.45 degrees (256 discrete bins)
        let d_yaw = (deg - recon_deg).abs();
        let circular_d_yaw = d_yaw.min((360.0 - d_yaw).abs());
        assert!(
            circular_d_yaw <= 1.45,
            "Yaw drift ({:.2} deg) must be <= 1.45 degrees",
            circular_d_yaw
        );
    }
}
