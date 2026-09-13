//! Core game mathematics, fixed-point vectors, coordinate quantization, and dead reckoning extrapolation algorithms.
//!
//! `eidolon-core` is a foundational, standalone crate designed with zero runtime dependencies.
//! It can be compiled for server runtimes or embedded into client game engines for deterministic prediction.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod authority;
pub mod fixed;
pub mod identity;
pub mod kinematics;
pub mod lock;
pub mod quant;
pub mod trace;

pub use authority::{AuthorityError, AuthorityFencer, AuthorityToken, AUTHORITY_TOKEN_LEN};
pub use fixed::{Fixed64, Vec3Fix};
pub use identity::{
    AccountId, CharacterAuthorization, CharacterId, IdentityError, IdentityRegistry, SessionTicket,
};
pub use kinematics::{
    extrapolate, reconcile_smooth, should_dispatch_update, DeadReckoningConfig, KinematicState,
    FLAG_FALLING, FLAG_IDLE, FLAG_IMMOBILIZED, FLAG_JUMPING, FLAG_SPRINTING, FLAG_WALKING,
};
pub use lock::{GenerationLockRegistry, LockEntry, LockError, LockToken, MAX_TRACKED_LOCKS};
pub use quant::{
    QuantizedCellCoord, QuantizedYaw, CELL_HORIZONTAL_SIZE, CELL_VERTICAL_SIZE,
    HORIZONTAL_RESOLUTION_METERS, MAX_QUANTIZED_HORIZONTAL, MAX_QUANTIZED_VERTICAL,
    VERTICAL_RESOLUTION_METERS,
};
pub use trace::{TraceContext, TraceRingBuffer, TraceSpan};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed64_basic_operations() {
        let a = Fixed64::from_i32(10);
        let b = Fixed64::from_i32(3);

        assert_eq!((a + b).to_i32(), 13);
        assert_eq!((a - b).to_i32(), 7);
        assert_eq!((a * b).to_i32(), 30);
        assert_eq!((a / b).to_i32(), 3);
    }

    #[test]
    fn test_fixed64_sqrt() {
        let val = Fixed64::from_f64(16.0);
        let root = val.sqrt();
        assert!((root.to_f64() - 4.0).abs() < 0.0001);

        let val_2 = Fixed64::from_f64(2.0);
        let root_2 = val_2.sqrt();
        assert!((root_2.to_f64() - core::f64::consts::SQRT_2).abs() < 0.0001);

        assert_eq!(Fixed64::ZERO.sqrt(), Fixed64::ZERO);
        assert_eq!(Fixed64::from_i32(-5).sqrt(), Fixed64::ZERO);
    }

    #[test]
    fn test_vec3fix_linear_algebra() {
        let v1 = Vec3Fix::new(
            Fixed64::from_i32(1),
            Fixed64::from_i32(2),
            Fixed64::from_i32(3),
        );
        let v2 = Vec3Fix::new(
            Fixed64::from_i32(4),
            Fixed64::from_i32(5),
            Fixed64::from_i32(6),
        );

        let sum = v1 + v2;
        assert_eq!(sum.x.to_i32(), 5);
        assert_eq!(sum.y.to_i32(), 7);
        assert_eq!(sum.z.to_i32(), 9);

        // Dot product: 1*4 + 2*5 + 3*6 = 4 + 10 + 18 = 32
        assert_eq!(v1.dot(v2).to_i32(), 32);

        // Cross product: (2*6 - 3*5, 3*4 - 1*6, 1*5 - 2*4) = (-3, 6, -3)
        let cross = v1.cross(v2);
        assert_eq!(cross.x.to_i32(), -3);
        assert_eq!(cross.y.to_i32(), 6);
        assert_eq!(cross.z.to_i32(), -3);
    }

    #[test]
    fn test_quantization_roundtrip() {
        let pos = Vec3Fix::from_f64(32.0, 16.0, 48.0);
        let quantized = QuantizedCellCoord::quantize(pos.x, pos.y, pos.z);

        assert!(quantized.is_valid());
        let reconstructed = quantized.dequantize();

        // Check sub-millimeter precision horizontally and <1cm elevation
        assert!((reconstructed.x.to_f64() - 32.0).abs() < 0.002);
        assert!((reconstructed.y.to_f64() - 16.0).abs() < 0.01);
        assert!((reconstructed.z.to_f64() - 48.0).abs() < 0.002);
    }

    #[test]
    fn test_global_quantization_roundtrip_multi_cell() {
        let test_positions = [
            Vec3Fix::from_f64(0.0, 0.0, 0.0),
            Vec3Fix::from_f64(10.5, 5.25, 20.75),
            Vec3Fix::from_f64(63.9, 31.9, 63.9),
            Vec3Fix::from_f64(64.1, 32.1, 64.1),
            Vec3Fix::from_f64(150.0, 100.0, 250.0),
            Vec3Fix::from_f64(-10.5, 12.0, -25.5),
            Vec3Fix::from_f64(-150.0, 50.0, -350.0),
        ];

        for pos in test_positions {
            let (cx, cy, cz, quant) = QuantizedCellCoord::quantize_from_global(pos);
            assert!(quant.is_valid());
            let reconstructed = QuantizedCellCoord::dequantize_to_global(cx, cy, cz, quant);

            // Sub-millimeter precision horizontally (< 1mm) and < 1cm vertically
            assert!(
                (reconstructed.x.to_f64() - pos.x.to_f64()).abs() < 0.002,
                "X mismatch for {pos:?}"
            );
            assert!(
                (reconstructed.y.to_f64() - pos.y.to_f64()).abs() < 0.01,
                "Y mismatch for {pos:?}"
            );
            assert!(
                (reconstructed.z.to_f64() - pos.z.to_f64()).abs() < 0.002,
                "Z mismatch for {pos:?}"
            );
        }
    }

    #[test]
    fn test_wire_bitpacking_7_bytes() {
        let original_coord = QuantizedCellCoord::new(12345, 2048, 54321);
        let original_yaw = QuantizedYaw::from_degrees(180.0);
        let original_flags = FLAG_SPRINTING | FLAG_JUMPING;

        let packed = original_coord.pack_with_yaw_and_flags(original_yaw, original_flags);
        assert_eq!(packed.len(), 7);

        let (unpacked_coord, unpacked_yaw, unpacked_flags) =
            QuantizedCellCoord::unpack_with_yaw_and_flags(packed);

        assert_eq!(unpacked_coord.x, original_coord.x);
        assert_eq!(unpacked_coord.y, original_coord.y);
        assert_eq!(unpacked_coord.z, original_coord.z);
        assert_eq!(unpacked_yaw, original_yaw);
        assert_eq!(unpacked_flags, original_flags & 0x0F);
    }

    #[test]
    fn test_yaw_shortest_arc_rollover() {
        let north = QuantizedYaw::NORTH; // 0
        let slightly_west = QuantizedYaw::from_byte(250); // 6 steps clockwise of North

        // Shortest arc from slightly_west to north should be +6 (turning counter-clockwise)
        assert_eq!(slightly_west.shortest_arc_delta(north), 6);
        // Shortest arc from north to slightly_west should be -6 (turning clockwise)
        assert_eq!(north.shortest_arc_delta(slightly_west), -6);
    }

    #[test]
    fn test_dead_reckoning_extrapolation() {
        let tick_duration = Fixed64::from_f64(0.05); // 20 Hz = 50ms
        let initial_state = KinematicState {
            position: Vec3Fix::ZERO,
            velocity: Vec3Fix::from_f64(10.0, 0.0, 0.0), // 10 m/s in X
            acceleration: Vec3Fix::ZERO,
            yaw: QuantizedYaw::EAST,
            angular_velocity: 0,
            flags: FLAG_WALKING,
        };

        // Extrapolate 20 ticks = 1.0 second
        let extrapolated = extrapolate(&initial_state, 20, tick_duration);
        assert!((extrapolated.position.x.to_f64() - 10.0).abs() < 0.01);
        assert_eq!(extrapolated.position.y, Fixed64::ZERO);
        assert_eq!(extrapolated.position.z, Fixed64::ZERO);
    }

    #[test]
    fn test_batch_distance_squared_4x_parity() {
        let target = Vec3Fix::from_f64(10.0, 5.0, 20.0);
        let origins = [
            Vec3Fix::from_f64(0.0, 0.0, 0.0),
            Vec3Fix::from_f64(10.0, 5.0, 20.0),
            Vec3Fix::from_f64(-5.0, 15.0, 30.0),
            Vec3Fix::from_f64(100.0, 50.0, 200.0),
        ];

        let batched = Vec3Fix::batch_distance_squared_4x(origins, target);

        for (i, origin) in origins.iter().enumerate() {
            let scalar = origin.distance_squared(target);
            assert_eq!(
                batched[i], scalar,
                "Batch distance lane {i} must match scalar distance"
            );
        }
    }

    #[test]
    fn test_reconcile_smooth_fractional_yaw() {
        use crate::kinematics::reconcile_smooth;

        let mut client_state = KinematicState {
            position: Vec3Fix::ZERO,
            velocity: Vec3Fix::ZERO,
            acceleration: Vec3Fix::ZERO,
            yaw: QuantizedYaw::from_byte(0),
            angular_velocity: 0,
            flags: 0,
        };

        let auth_state = KinematicState {
            position: Vec3Fix::ZERO,
            velocity: Vec3Fix::ZERO,
            acceleration: Vec3Fix::ZERO,
            yaw: QuantizedYaw::from_byte(40), // 40 steps counter-clockwise
            angular_velocity: 0,
            flags: 0,
        };

        // 25% smoothing should advance by 10 steps (40 * 0.25 = 10)
        reconcile_smooth(&mut client_state, &auth_state, Fixed64::from_f64(0.25));
        assert_eq!(client_state.yaw.as_byte(), 10);

        // Another 50% smoothing from 10 to 40 (delta 30) should advance by 15 steps -> 25
        reconcile_smooth(&mut client_state, &auth_state, Fixed64::from_f64(0.50));
        assert_eq!(client_state.yaw.as_byte(), 25);

        // When yaw is already synchronized (delta = 0), yaw must remain unchanged (no 1-step jitter)
        let synced_auth = client_state;
        reconcile_smooth(&mut client_state, &synced_auth, Fixed64::from_f64(0.50));
        assert_eq!(client_state.yaw.as_byte(), 25);
    }
}
