//! Integration test suite for Phase 29: Spatial Audio Propagation, Acoustic Occlusion & P2P Voice Routing.
//!
//! Validates distance attenuation models, 3D stereo panning azimuth calculations,
//! compound structure acoustic raycasting occlusion, frequency muffling,
//! and zero-allocation P2P voice mesh routing descriptors.

use eidolon_core::fixed::Vec3Fix;
use eidolon_core::quant::QuantizedYaw;
use eidolon_core::structure::PieceType;
use eidolon_spatial::bvh::CompoundStructure;
use eidolon_world::voice_router::{
    AttenuationModel, SpatialVoiceManager, VoiceEmitter, VoiceError, VoiceListener,
    VoiceRoutingUpdate, MAX_EMITTERS, MAX_LISTENERS, MAX_VOICE_CHANNELS_PER_LISTENER,
};

#[test]
fn test_attenuation_models_mathematical_precision() {
    // 1. Inverse Distance Model
    let inv = AttenuationModel::InverseDistance {
        min_distance: 2.0,
        max_distance: 30.0,
        rolloff: 1.0,
    };

    // At or inside min_distance -> full gain 1.0
    assert_eq!(inv.calculate_gain(0.0), 1.0);
    assert_eq!(inv.calculate_gain(1.0), 1.0);
    assert_eq!(inv.calculate_gain(2.0), 1.0);

    // At halfway (e.g. distance = 4.0 -> d = 2.0, denom = 2.0 + 1.0 * 2.0 = 4.0 -> gain = 0.5)
    let halfway_gain = inv.calculate_gain(4.0);
    assert!((halfway_gain - 0.5).abs() < 0.001);

    // Beyond max_distance -> zero gain
    assert_eq!(inv.calculate_gain(30.0), 0.0);
    assert_eq!(inv.calculate_gain(50.0), 0.0);

    // Boundary edge cases: negative, NaN
    assert_eq!(inv.calculate_gain(-5.0), 0.0);
    assert_eq!(inv.calculate_gain(f32::NAN), 0.0);

    // 2. Linear Model
    let lin = AttenuationModel::Linear {
        min_distance: 5.0,
        max_distance: 25.0,
    };

    assert_eq!(lin.calculate_gain(2.0), 1.0);
    assert_eq!(lin.calculate_gain(5.0), 1.0);
    // At distance = 15.0 -> exactly 0.5
    assert!((lin.calculate_gain(15.0) - 0.5).abs() < 0.001);
    assert_eq!(lin.calculate_gain(25.0), 0.0);
    assert_eq!(lin.calculate_gain(30.0), 0.0);

    // 3. Exponential Model
    let exp = AttenuationModel::Exponential {
        min_distance: 0.0,
        max_distance: 10.0,
        exponent: 2.0,
    };

    assert_eq!(exp.calculate_gain(0.0), 1.0);
    // At distance = 5.0 -> (5/10)^2 = 0.25
    assert!((exp.calculate_gain(5.0) - 0.25).abs() < 0.001);
    assert_eq!(exp.calculate_gain(10.0), 0.0);
}

#[test]
fn test_spatial_panning_azimuth_precision() {
    let listener_pos = Vec3Fix::from_f64(0.0, 0.0, 0.0);

    // Listener facing North (+Z, yaw = 0)
    let yaw_north = QuantizedYaw::from_degrees(0.0);

    // Emitter directly ahead North (+Z) -> pan = 0.0 (center)
    let emitter_north = Vec3Fix::from_f64(0.0, 0.0, 10.0);
    let pan_north =
        SpatialVoiceManager::calculate_spatial_panning(listener_pos, yaw_north, emitter_north);
    assert!(
        pan_north.abs() < 0.01,
        "Expected center pan, got {}",
        pan_north
    );

    // Emitter directly East (+X) -> pan = +1.0 (full right)
    let emitter_east = Vec3Fix::from_f64(10.0, 0.0, 0.0);
    let pan_east =
        SpatialVoiceManager::calculate_spatial_panning(listener_pos, yaw_north, emitter_east);
    assert!(
        (pan_east - 1.0).abs() < 0.01,
        "Expected full right (+1.0), got {}",
        pan_east
    );

    // Emitter directly West (-X) -> pan = -1.0 (full left)
    let emitter_west = Vec3Fix::from_f64(-10.0, 0.0, 0.0);
    let pan_west =
        SpatialVoiceManager::calculate_spatial_panning(listener_pos, yaw_north, emitter_west);
    assert!(
        (pan_west - (-1.0)).abs() < 0.01,
        "Expected full left (-1.0), got {}",
        pan_west
    );

    // Emitter directly South (-Z) -> pan = 0.0 (behind center)
    let emitter_south = Vec3Fix::from_f64(0.0, 0.0, -10.0);
    let pan_south =
        SpatialVoiceManager::calculate_spatial_panning(listener_pos, yaw_north, emitter_south);
    assert!(
        pan_south.abs() < 0.01,
        "Expected center pan behind, got {}",
        pan_south
    );

    // Rotate listener 90 degrees to face East (+X, yaw = 90 deg -> 64 quantized units)
    let yaw_east = QuantizedYaw::from_degrees(90.0);

    // When facing East, North (+Z) is to the listener's left!
    let pan_north_rotated =
        SpatialVoiceManager::calculate_spatial_panning(listener_pos, yaw_east, emitter_north);
    assert!(
        (pan_north_rotated - (-1.0)).abs() < 0.05,
        "Expected left pan when facing East, got {}",
        pan_north_rotated
    );
}

#[test]
fn test_structure_acoustic_occlusion_and_low_pass_muffling() {
    let origin = Vec3Fix::from_f64(0.0, 1.5, 0.0); // Speaker at origin, mouth height 1.5m
    let target = Vec3Fix::from_f64(10.0, 1.5, 0.0); // Listener at X=10m

    // 1. Unobstructed scenario (empty structures)
    let empty_structures: Vec<CompoundStructure> = Vec::new();
    let unobstructed = SpatialVoiceManager::calculate_occlusion(origin, target, &empty_structures);
    assert!(!unobstructed.is_occluded);
    assert_eq!(unobstructed.occlusion_db, 0.0);
    assert_eq!(unobstructed.low_pass_cutoff_hz, 20000);
    assert_eq!(unobstructed.hit_count, 0);

    // 2. Solid wall placed at X=5.0
    let mut house = CompoundStructure::new(101);
    // Wall center at (5.0, 1.5, 0.0)
    house.add_piece(1, PieceType::Wall, Vec3Fix::from_f64(5.0, 1.5, 0.0));

    let occluded = SpatialVoiceManager::calculate_occlusion(origin, target, &[house.clone()]);
    assert!(occluded.is_occluded);
    assert_eq!(occluded.hit_count, 1);
    assert_eq!(occluded.occlusion_db, -18.0); // Stone wall transmission loss
    assert_eq!(occluded.low_pass_cutoff_hz, 500); // 500 Hz low-pass cutoff

    // 3. Window opening wall instead of solid wall
    let mut window_house = CompoundStructure::new(102);
    window_house.add_piece(2, PieceType::WindowWall, Vec3Fix::from_f64(5.0, 1.5, 0.0));

    let window_occluded = SpatialVoiceManager::calculate_occlusion(origin, target, &[window_house]);
    assert!(window_occluded.is_occluded);
    assert_eq!(window_occluded.hit_count, 1);
    assert_eq!(window_occluded.occlusion_db, -3.5); // Open window allows sound diffraction
    assert_eq!(window_occluded.low_pass_cutoff_hz, 3200); // 3200 Hz cutoff (crisper audio)

    // 4. Double wall obstacle (interior room partitioning)
    let mut double_wall_house = CompoundStructure::new(103);
    double_wall_house.add_piece(3, PieceType::Wall, Vec3Fix::from_f64(3.0, 1.5, 0.0));
    double_wall_house.add_piece(4, PieceType::Wall, Vec3Fix::from_f64(7.0, 1.5, 0.0));

    let double_occluded =
        SpatialVoiceManager::calculate_occlusion(origin, target, &[double_wall_house]);
    assert!(double_occluded.is_occluded);
    assert_eq!(double_occluded.hit_count, 2);
    assert_eq!(double_occluded.occlusion_db, -36.0); // -18 dB * 2
    assert_eq!(double_occluded.low_pass_cutoff_hz, 500);
}

#[test]
fn test_voice_mesh_evaluation_and_p2p_channel_saturation() {
    let mut voice_manager = SpatialVoiceManager::new();

    // Register 1 listener at (0, 0, 0)
    let listener = VoiceListener::new(
        1,
        Vec3Fix::from_f64(0.0, 0.0, 0.0),
        QuantizedYaw::from_degrees(0.0),
    );
    assert!(voice_manager.register_listener(listener).is_ok());

    // Register 12 speaking emitters at various distances: 2m, 4m, 6m, 8m, ..., 24m
    for i in 1..=12 {
        let entity_id = 100 + i as u64;
        let dist = i as f64 * 2.0;
        let mut emitter = VoiceEmitter::new(entity_id, Vec3Fix::from_f64(0.0, 0.0, dist));
        emitter.is_speaking = true;
        emitter.voice_amplitude = 1.0;
        assert!(voice_manager.register_emitter(emitter).is_ok());
    }

    let mut updates = [VoiceRoutingUpdate::new(0); 4];
    let empty_structures: Vec<CompoundStructure> = Vec::new();
    let processed = voice_manager.evaluate_voice_mesh(&empty_structures, &mut updates);

    assert_eq!(processed, 1);
    let update = &updates[0];
    assert_eq!(update.listener_id, 1);
    // Bounded to MAX_VOICE_CHANNELS_PER_LISTENER (8 channels)
    assert_eq!(update.channel_count, MAX_VOICE_CHANNELS_PER_LISTENER);

    // Channels must be sorted descending by perceived gain (loudest/closest first)
    for i in 0..MAX_VOICE_CHANNELS_PER_LISTENER - 1 {
        let curr = update.channels[i].as_ref().unwrap();
        let next = update.channels[i + 1].as_ref().unwrap();
        assert!(
            curr.gain >= next.gain,
            "Channels must be sorted by gain descending: {} vs {}",
            curr.gain,
            next.gain
        );
        // Emitter at 2m (speaker 101) should be louder than emitter at 4m (speaker 102)
        assert!(curr.distance_meters <= next.distance_meters);
    }

    // Verify deterministic P2P tokens are populated and non-zero
    for ch in update.channels.iter().flatten() {
        assert_ne!(ch.p2p_token, 0);
    }

    // Test that silent emitter is excluded
    assert!(voice_manager
        .update_emitter_state(101, false, 0.0, Vec3Fix::from_f64(0.0, 0.0, 2.0))
        .is_ok());
    let processed_after_mute = voice_manager.evaluate_voice_mesh(&empty_structures, &mut updates);
    assert_eq!(processed_after_mute, 1);
    let update_after = &updates[0];
    // Speaker 101 should no longer be in any channel
    for ch in update_after.channels.iter().flatten() {
        assert_ne!(ch.speaker_id, 101);
    }
}

#[test]
fn test_voice_manager_lifecycle_and_capacity_limits() {
    let mut voice_manager = SpatialVoiceManager::new();

    // 1. Fill all listener slots up to MAX_LISTENERS (64)
    for i in 0..MAX_LISTENERS {
        let listener = VoiceListener::new(
            i as u64 + 1,
            Vec3Fix::from_f64(i as f64, 0.0, 0.0),
            QuantizedYaw::from_degrees(0.0),
        );
        assert_eq!(voice_manager.register_listener(listener), Ok(()));
    }
    assert_eq!(voice_manager.listener_count(), MAX_LISTENERS);

    // 65th listener should trigger ListenerLimitReached
    let overflow_listener = VoiceListener::new(999, Vec3Fix::ZERO, QuantizedYaw::from_degrees(0.0));
    assert_eq!(
        voice_manager.register_listener(overflow_listener),
        Err(VoiceError::ListenerLimitReached)
    );

    // Unregister one listener and re-register
    assert!(voice_manager.unregister_listener(10));
    assert_eq!(voice_manager.listener_count(), MAX_LISTENERS - 1);
    assert_eq!(voice_manager.register_listener(overflow_listener), Ok(()));
    assert_eq!(voice_manager.listener_count(), MAX_LISTENERS);

    // 2. Fill all emitter slots up to MAX_EMITTERS (64)
    for i in 0..MAX_EMITTERS {
        let emitter = VoiceEmitter::new(i as u64 + 100, Vec3Fix::from_f64(i as f64, 0.0, 0.0));
        assert_eq!(voice_manager.register_emitter(emitter), Ok(()));
    }
    assert_eq!(voice_manager.emitter_count(), MAX_EMITTERS);

    // 65th emitter should trigger EmitterLimitReached
    let overflow_emitter = VoiceEmitter::new(9999, Vec3Fix::ZERO);
    assert_eq!(
        voice_manager.register_emitter(overflow_emitter),
        Err(VoiceError::EmitterLimitReached)
    );

    // Update non-existent entities returns typed error
    assert_eq!(
        voice_manager.update_emitter_state(8888, true, 1.0, Vec3Fix::ZERO),
        Err(VoiceError::EmitterNotFound)
    );
    assert_eq!(
        voice_manager.update_listener_state(8888, Vec3Fix::ZERO, QuantizedYaw::from_degrees(0.0)),
        Err(VoiceError::ListenerNotFound)
    );
}
