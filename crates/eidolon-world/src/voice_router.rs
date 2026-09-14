//! Spatial audio propagation, acoustic occlusion raycasting, and P2P voice mesh routing.
//!
//! Provides deterministic 3D positional audio attenuation, structure-aware acoustic
//! raycasting with frequency low-pass muffling, and zero-allocation peer-to-peer (P2P)
//! voice routing descriptors with zero external third-party dependencies.

#![deny(unsafe_code)]

use eidolon_core::fixed::Vec3Fix;
use eidolon_core::quant::QuantizedYaw;
use eidolon_core::structure::PieceType;
use eidolon_spatial::bvh::CompoundStructure;

/// Maximum concurrent audible voice channels tracked per listener.
pub const MAX_VOICE_CHANNELS_PER_LISTENER: usize = 8;

/// Maximum registered listeners in a single zone voice manager.
pub const MAX_LISTENERS: usize = 64;

/// Maximum registered emitters in a single zone voice manager.
pub const MAX_EMITTERS: usize = 64;

/// Default maximum audible distance in meters before voice drops to zero gain.
pub const DEFAULT_MAX_AUDIBLE_DISTANCE: f32 = 35.0;

/// Default minimum audible distance in meters for maximum volume before attenuation begins.
pub const DEFAULT_MIN_AUDIBLE_DISTANCE: f32 = 1.5;

/// High-frequency air absorption attenuation in decibels per meter.
pub const AIR_ABSORPTION_DB_PER_METER: f32 = 0.05;

/// Inaudibility threshold below which audio streams are dropped to conserve bandwidth (-50 dB).
pub const INAUDIBLE_GAIN_THRESHOLD: f32 = 0.00316;

/// Material classification determining sound absorption and low-pass filter muffling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcousticMaterial {
    /// Thick stone foundation or masonry slab (-24 dB transmission loss, 400 Hz cutoff).
    Stone,
    /// Timber or wooden paneling (-12 dB transmission loss, 800 Hz cutoff).
    Wood,
    /// Solid metal plating or reinforced iron (-26 dB transmission loss, 350 Hz cutoff).
    Metal,
    /// Open door frame, window opening, or archway (-3 dB transmission loss, 3000 Hz cutoff).
    OpenOpening,
    /// Foliage, bushes, or organic canopy (-1.5 dB transmission loss, 5000 Hz cutoff).
    Foliage,
}

impl AcousticMaterial {
    /// Maps a building piece type to its representative acoustic material.
    pub fn from_piece_type(piece: PieceType) -> Self {
        match piece {
            PieceType::Foundation => Self::Stone,
            PieceType::Wall => Self::Stone,
            PieceType::DoorFrame | PieceType::WindowWall => Self::OpenOpening,
            PieceType::Floor | PieceType::Roof | PieceType::Stairs | PieceType::Pillar => {
                Self::Wood
            }
        }
    }

    /// Returns the acoustic transmission loss in decibels (dB) through this material.
    pub fn transmission_loss_db(&self) -> f32 {
        match self {
            Self::Stone => -18.0,
            Self::Wood => -10.0,
            Self::Metal => -24.0,
            Self::OpenOpening => -3.5,
            Self::Foliage => -1.5,
        }
    }

    /// Returns the low-pass filter cutoff frequency in Hertz (Hz) caused by obstacle muffling.
    pub fn cutoff_frequency_hz(&self) -> u16 {
        match self {
            Self::Stone => 500,
            Self::Wood => 900,
            Self::Metal => 400,
            Self::OpenOpening => 3200,
            Self::Foliage => 5500,
        }
    }
}

/// Positional distance attenuation mathematical model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AttenuationModel {
    /// Inverse distance rolloff: gain = min_dist / (min_dist + rolloff * (dist - min_dist)).
    InverseDistance {
        /// Distance at which full volume is preserved.
        min_distance: f32,
        /// Cutoff distance beyond which audio is inaudible.
        max_distance: f32,
        /// Rolloff coefficient steepness.
        rolloff: f32,
    },
    /// Linear distance rolloff: gain = 1.0 - (dist - min_dist) / (max_dist - min_dist).
    Linear {
        /// Distance at which full volume is preserved.
        min_distance: f32,
        /// Cutoff distance beyond which audio is inaudible.
        max_distance: f32,
    },
    /// Exponential distance rolloff: gain = ((max_dist - dist) / (max_dist - min_dist))^exponent.
    Exponential {
        /// Distance at which full volume is preserved.
        min_distance: f32,
        /// Cutoff distance beyond which audio is inaudible.
        max_distance: f32,
        /// Exponential falloff power.
        exponent: f32,
    },
}

impl Default for AttenuationModel {
    fn default() -> Self {
        Self::InverseDistance {
            min_distance: DEFAULT_MIN_AUDIBLE_DISTANCE,
            max_distance: DEFAULT_MAX_AUDIBLE_DISTANCE,
            rolloff: 1.0,
        }
    }
}

impl AttenuationModel {
    /// Calculates the scalar distance gain factor (0.0 to 1.0) for a given distance in meters.
    pub fn calculate_gain(&self, distance: f32) -> f32 {
        if distance.is_nan() || distance < 0.0 {
            return 0.0;
        }

        match *self {
            Self::InverseDistance {
                min_distance,
                max_distance,
                rolloff,
            } => {
                if distance <= min_distance {
                    1.0
                } else if distance >= max_distance {
                    0.0
                } else {
                    let d = distance - min_distance;
                    let denom = min_distance + rolloff.max(0.01) * d;
                    if denom <= 0.0 {
                        0.0
                    } else {
                        (min_distance / denom).clamp(0.0, 1.0)
                    }
                }
            }
            Self::Linear {
                min_distance,
                max_distance,
            } => {
                if distance <= min_distance {
                    1.0
                } else if distance >= max_distance {
                    0.0
                } else {
                    let range = max_distance - min_distance;
                    if range <= 0.0 {
                        0.0
                    } else {
                        (1.0 - (distance - min_distance) / range).clamp(0.0, 1.0)
                    }
                }
            }
            Self::Exponential {
                min_distance,
                max_distance,
                exponent,
            } => {
                if distance <= min_distance {
                    1.0
                } else if distance >= max_distance {
                    0.0
                } else {
                    let range = max_distance - min_distance;
                    if range <= 0.0 {
                        0.0
                    } else {
                        let normalized = ((max_distance - distance) / range).clamp(0.0, 1.0);
                        normalized.powf(exponent.max(0.1))
                    }
                }
            }
        }
    }

    /// Returns the maximum hearing distance limit in meters.
    pub fn max_distance(&self) -> f32 {
        match *self {
            Self::InverseDistance { max_distance, .. } => max_distance,
            Self::Linear { max_distance, .. } => max_distance,
            Self::Exponential { max_distance, .. } => max_distance,
        }
    }
}

/// Active voice emitting entity (e.g. speaking player character).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceEmitter {
    /// Unique entity identifier of the speaker.
    pub entity_id: u64,
    /// Current 32.32 fixed-point continuous world position.
    pub position: Vec3Fix,
    /// True when voice activity detection (VAD) indicates microphone input.
    pub is_speaking: bool,
    /// Measured microphone input volume amplitude (0.0 to 1.0).
    pub voice_amplitude: f32,
    /// Positional distance attenuation configuration.
    pub attenuation_model: AttenuationModel,
}

impl VoiceEmitter {
    /// Creates a new voice emitter with default inverse distance attenuation.
    pub fn new(entity_id: u64, position: Vec3Fix) -> Self {
        Self {
            entity_id,
            position,
            is_speaking: false,
            voice_amplitude: 1.0,
            attenuation_model: AttenuationModel::default(),
        }
    }
}

/// Active voice listener entity (e.g. observing player character).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceListener {
    /// Unique entity identifier of the listener.
    pub entity_id: u64,
    /// Current 32.32 fixed-point continuous world position.
    pub position: Vec3Fix,
    /// Quantized yaw orientation used for 3D stereo panning azimuth calculation.
    pub yaw: QuantizedYaw,
    /// Hearing sensitivity multiplier (default 1.0).
    pub hearing_sensitivity: f32,
}

impl VoiceListener {
    /// Creates a new voice listener.
    pub fn new(entity_id: u64, position: Vec3Fix, yaw: QuantizedYaw) -> Self {
        Self {
            entity_id,
            position,
            yaw,
            hearing_sensitivity: 1.0,
        }
    }
}

/// Result of an acoustic raycast query between emitter and listener.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AcousticOcclusionResult {
    /// True if any solid building pieces intersected the direct line-of-sight sound ray.
    pub is_occluded: bool,
    /// Cumulative decibel attenuation from penetrating structural obstacles.
    pub occlusion_db: f32,
    /// Lowest low-pass filter cutoff frequency in Hertz across all intersected materials.
    pub low_pass_cutoff_hz: u16,
    /// Total count of structural pieces penetrated by the acoustic ray.
    pub hit_count: u8,
}

impl Default for AcousticOcclusionResult {
    fn default() -> Self {
        Self {
            is_occluded: false,
            occlusion_db: 0.0,
            low_pass_cutoff_hz: 20000,
            hit_count: 0,
        }
    }
}

/// Directed peer-to-peer audio routing descriptor sent to client.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoicePeerDescriptor {
    /// Entity ID of the remote speaker.
    pub speaker_id: u64,
    /// Effective audio gain multiplier (0.0 to 1.0) incorporating distance and occlusion.
    pub gain: f32,
    /// Stereo horizontal pan (-1.0 full left, 0.0 center, +1.0 full right).
    pub pan: f32,
    /// Acoustic low-pass muffling cutoff frequency in Hertz.
    pub low_pass_cutoff_hz: u16,
    /// Decibel transmission loss from structural obstacles.
    pub occlusion_db: f32,
    /// Euclidean distance between listener and speaker in meters.
    pub distance_meters: f32,
    /// Deterministic 64-bit pairing authorization token for direct P2P datagram connection.
    pub p2p_token: u64,
}

/// Per-listener voice routing instruction containing the top audible channels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceRoutingUpdate {
    /// Entity ID of the target listener.
    pub listener_id: u64,
    /// Fixed-capacity array of top audible peer channels sorted by perceived loudness.
    pub channels: [Option<VoicePeerDescriptor>; MAX_VOICE_CHANNELS_PER_LISTENER],
    /// Number of active channels in the array.
    pub channel_count: usize,
}

impl VoiceRoutingUpdate {
    /// Creates a new empty routing update for a specific listener.
    pub fn new(listener_id: u64) -> Self {
        Self {
            listener_id,
            channels: [None; MAX_VOICE_CHANNELS_PER_LISTENER],
            channel_count: 0,
        }
    }
}

/// Error conditions encountered in spatial voice management.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceError {
    /// Listener registration exceeded fixed pre-allocated capacity limit.
    ListenerLimitReached,
    /// Emitter registration exceeded fixed pre-allocated capacity limit.
    EmitterLimitReached,
    /// Specified listener entity ID was not found.
    ListenerNotFound,
    /// Specified emitter entity ID was not found.
    EmitterNotFound,
    /// Invalid parameter value supplied.
    InvalidParameter(&'static str),
}

/// Authoritative spatial voice propagation and P2P routing broker.
///
/// Pre-allocates all listener and emitter slots with contiguous memory and zero dynamic
/// heap allocations during live simulation ticks. Evaluates distance attenuation,
/// structure acoustic raycasting, azimuth panning, and generates direct P2P mesh pairing descriptors.
pub struct SpatialVoiceManager {
    listeners: [Option<VoiceListener>; MAX_LISTENERS],
    emitters: [Option<VoiceEmitter>; MAX_EMITTERS],
    listener_count: usize,
    emitter_count: usize,
}

impl Default for SpatialVoiceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SpatialVoiceManager {
    /// Creates a new spatial voice manager with empty pre-allocated slots.
    pub fn new() -> Self {
        Self {
            listeners: [None; MAX_LISTENERS],
            emitters: [None; MAX_EMITTERS],
            listener_count: 0,
            emitter_count: 0,
        }
    }

    /// Returns the number of currently registered listeners.
    pub fn listener_count(&self) -> usize {
        self.listener_count
    }

    /// Returns the number of currently registered emitters.
    pub fn emitter_count(&self) -> usize {
        self.emitter_count
    }

    /// Registers a listener. Returns an error if the fixed capacity is reached.
    pub fn register_listener(&mut self, listener: VoiceListener) -> Result<(), VoiceError> {
        // Check if already registered to update in-place
        for existing in self.listeners.iter_mut().flatten() {
            if existing.entity_id == listener.entity_id {
                *existing = listener;
                return Ok(());
            }
        }

        // Find empty slot
        for slot in self.listeners.iter_mut() {
            if slot.is_none() {
                *slot = Some(listener);
                self.listener_count += 1;
                return Ok(());
            }
        }

        Err(VoiceError::ListenerLimitReached)
    }

    /// Unregisters a listener by entity ID.
    pub fn unregister_listener(&mut self, entity_id: u64) -> bool {
        for slot in self.listeners.iter_mut() {
            if let Some(listener) = slot {
                if listener.entity_id == entity_id {
                    *slot = None;
                    self.listener_count = self.listener_count.saturating_sub(1);
                    return true;
                }
            }
        }
        false
    }

    /// Registers an emitter. Returns an error if the fixed capacity is reached.
    pub fn register_emitter(&mut self, emitter: VoiceEmitter) -> Result<(), VoiceError> {
        // Check if already registered to update in-place
        for existing in self.emitters.iter_mut().flatten() {
            if existing.entity_id == emitter.entity_id {
                *existing = emitter;
                return Ok(());
            }
        }

        // Find empty slot
        for slot in self.emitters.iter_mut() {
            if slot.is_none() {
                *slot = Some(emitter);
                self.emitter_count += 1;
                return Ok(());
            }
        }

        Err(VoiceError::EmitterLimitReached)
    }

    /// Unregisters an emitter by entity ID.
    pub fn unregister_emitter(&mut self, entity_id: u64) -> bool {
        for slot in self.emitters.iter_mut() {
            if let Some(emitter) = slot {
                if emitter.entity_id == entity_id {
                    *slot = None;
                    self.emitter_count = self.emitter_count.saturating_sub(1);
                    return true;
                }
            }
        }
        false
    }

    /// Updates voice activity state and position for an existing emitter.
    pub fn update_emitter_state(
        &mut self,
        entity_id: u64,
        is_speaking: bool,
        amplitude: f32,
        position: Vec3Fix,
    ) -> Result<(), VoiceError> {
        for emitter in self.emitters.iter_mut().flatten() {
            if emitter.entity_id == entity_id {
                emitter.is_speaking = is_speaking;
                emitter.voice_amplitude = amplitude.clamp(0.0, 1.0);
                emitter.position = position;
                return Ok(());
            }
        }
        Err(VoiceError::EmitterNotFound)
    }

    /// Updates listener position and orientation for an existing listener.
    pub fn update_listener_state(
        &mut self,
        entity_id: u64,
        position: Vec3Fix,
        yaw: QuantizedYaw,
    ) -> Result<(), VoiceError> {
        for listener in self.listeners.iter_mut().flatten() {
            if listener.entity_id == entity_id {
                listener.position = position;
                listener.yaw = yaw;
                return Ok(());
            }
        }
        Err(VoiceError::ListenerNotFound)
    }

    /// Evaluates acoustic occlusion between sound origin and listener target against compound structures.
    ///
    /// Tests line segment between origin and target against all leaf building pieces within
    /// structures, accumulating decibel attenuation and finding the lowest low-pass filter frequency cutoff.
    pub fn calculate_occlusion(
        origin: Vec3Fix,
        target: Vec3Fix,
        structures: &[CompoundStructure],
    ) -> AcousticOcclusionResult {
        if structures.is_empty() {
            return AcousticOcclusionResult::default();
        }

        let delta = target - origin;
        let dx = delta.x.to_f64();
        let dy = delta.y.to_f64();
        let dz = delta.z.to_f64();
        let total_dist = (dx * dx + dy * dy + dz * dz).sqrt();

        if total_dist < 0.01 {
            return AcousticOcclusionResult::default();
        }

        let dir = Vec3Fix::from_f64(dx / total_dist, dy / total_dist, dz / total_dist);

        let mut total_occlusion_db = 0.0f32;
        let mut min_cutoff_hz = 20000u16;
        let mut hit_count = 0u8;

        const MAX_HITS: u8 = 8;

        for structure in structures {
            // Broad-phase: test outer compound AABB first
            if !Self::segment_intersects_bounds(
                origin,
                dir,
                total_dist,
                structure.min_bounds,
                structure.max_bounds,
            ) {
                continue;
            }

            // Narrow-phase: test individual leaf pieces
            for piece in &structure.pieces {
                if hit_count >= MAX_HITS {
                    break;
                }

                if Self::segment_intersects_bounds(
                    origin,
                    dir,
                    total_dist,
                    piece.min_bounds,
                    piece.max_bounds,
                ) {
                    let material = AcousticMaterial::from_piece_type(piece.piece_type);
                    total_occlusion_db += material.transmission_loss_db();
                    min_cutoff_hz = min_cutoff_hz.min(material.cutoff_frequency_hz());
                    hit_count += 1;
                }
            }

            if hit_count >= MAX_HITS {
                break;
            }
        }

        AcousticOcclusionResult {
            is_occluded: hit_count > 0,
            occlusion_db: total_occlusion_db,
            low_pass_cutoff_hz: min_cutoff_hz,
            hit_count,
        }
    }

    /// Tests if a line segment [origin, origin + dir * length] intersects an AABB.
    fn segment_intersects_bounds(
        origin: Vec3Fix,
        dir: Vec3Fix,
        length: f64,
        min_b: Vec3Fix,
        max_b: Vec3Fix,
    ) -> bool {
        let orig_x = origin.x.to_f64();
        let orig_y = origin.y.to_f64();
        let orig_z = origin.z.to_f64();

        let dir_x = dir.x.to_f64();
        let dir_y = dir.y.to_f64();
        let dir_z = dir.z.to_f64();

        let min_x = min_b.x.to_f64();
        let min_y = min_b.y.to_f64();
        let min_z = min_b.z.to_f64();

        let max_x = max_b.x.to_f64();
        let max_y = max_b.y.to_f64();
        let max_z = max_b.z.to_f64();

        let mut tmin = 0.0f64;
        let mut tmax = length;

        // X axis slab
        if dir_x.abs() > 1e-6 {
            let inv_d = 1.0 / dir_x;
            let mut t1 = (min_x - orig_x) * inv_d;
            let mut t2 = (max_x - orig_x) * inv_d;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            tmin = tmin.max(t1);
            tmax = tmax.min(t2);
            if tmin > tmax {
                return false;
            }
        } else if orig_x < min_x || orig_x > max_x {
            return false;
        }

        // Y axis slab
        if dir_y.abs() > 1e-6 {
            let inv_d = 1.0 / dir_y;
            let mut t1 = (min_y - orig_y) * inv_d;
            let mut t2 = (max_y - orig_y) * inv_d;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            tmin = tmin.max(t1);
            tmax = tmax.min(t2);
            if tmin > tmax {
                return false;
            }
        } else if orig_y < min_y || orig_y > max_y {
            return false;
        }

        // Z axis slab
        if dir_z.abs() > 1e-6 {
            let inv_d = 1.0 / dir_z;
            let mut t1 = (min_z - orig_z) * inv_d;
            let mut t2 = (max_z - orig_z) * inv_d;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            tmin = tmin.max(t1);
            tmax = tmax.min(t2);
            if tmin > tmax {
                return false;
            }
        } else if orig_z < min_z || orig_z > max_z {
            return false;
        }

        tmax >= 0.0 && tmin <= length
    }

    /// Computes horizontal stereo panning factor (-1.0 to +1.0) based on listener yaw orientation.
    pub fn calculate_spatial_panning(
        listener_pos: Vec3Fix,
        listener_yaw: QuantizedYaw,
        emitter_pos: Vec3Fix,
    ) -> f32 {
        let delta = emitter_pos - listener_pos;
        let dx = delta.x.to_f64() as f32;
        let dz = delta.z.to_f64() as f32;

        let dist_sq = dx * dx + dz * dz;
        if dist_sq < 0.0001 {
            return 0.0;
        }

        // World angle to emitter (0 rad = +Z, PI/2 = +X)
        let emitter_angle = dx.atan2(dz);
        let listener_angle = listener_yaw.to_radians() as f32;

        let mut rel_angle = emitter_angle - listener_angle;
        while rel_angle > std::f32::consts::PI {
            rel_angle -= 2.0 * std::f32::consts::PI;
        }
        while rel_angle < -std::f32::consts::PI {
            rel_angle += 2.0 * std::f32::consts::PI;
        }

        rel_angle.sin().clamp(-1.0, 1.0)
    }

    /// Evaluates the spatial voice mesh for all active listeners against active emitters.
    ///
    /// Populates `out_updates` with up to `MAX_VOICE_CHANNELS_PER_LISTENER` loudest audible channels
    /// per listener with zero heap allocations. Returns the total number of listeners processed.
    pub fn evaluate_voice_mesh(
        &self,
        structures: &[CompoundStructure],
        out_updates: &mut [VoiceRoutingUpdate],
    ) -> usize {
        let mut processed_listeners = 0;

        for listener_slot in self.listeners.iter().flatten() {
            if processed_listeners >= out_updates.len() {
                break;
            }

            let mut update = VoiceRoutingUpdate::new(listener_slot.entity_id);
            let mut candidate_count = 0usize;
            let mut candidates: [Option<VoicePeerDescriptor>; MAX_VOICE_CHANNELS_PER_LISTENER] =
                [None; MAX_VOICE_CHANNELS_PER_LISTENER];

            for emitter_slot in self.emitters.iter().flatten() {
                // Skip self-hearing
                if emitter_slot.entity_id == listener_slot.entity_id {
                    continue;
                }

                // Skip non-speaking or silent emitters
                if !emitter_slot.is_speaking || emitter_slot.voice_amplitude <= 0.001 {
                    continue;
                }

                // Calculate Euclidean distance
                let delta = emitter_slot.position - listener_slot.position;
                let dx = delta.x.to_f64() as f32;
                let dy = delta.y.to_f64() as f32;
                let dz = delta.z.to_f64() as f32;
                let dist = (dx * dx + dy * dy + dz * dz).sqrt();

                if dist > emitter_slot.attenuation_model.max_distance() {
                    continue;
                }

                // Distance gain
                let distance_gain = emitter_slot.attenuation_model.calculate_gain(dist);
                if distance_gain <= 0.0 {
                    continue;
                }

                // Acoustic structure occlusion
                let occlusion = Self::calculate_occlusion(
                    emitter_slot.position,
                    listener_slot.position,
                    structures,
                );

                // Convert decibels to linear amplitude: gain = 10^(dB / 20)
                let occlusion_gain = 10.0f32.powf(occlusion.occlusion_db / 20.0);

                // Air absorption over distance
                let air_absorption_db = -AIR_ABSORPTION_DB_PER_METER * dist;
                let air_gain = 10.0f32.powf(air_absorption_db / 20.0);

                // Effective perceived gain
                let effective_gain = distance_gain
                    * occlusion_gain
                    * air_gain
                    * emitter_slot.voice_amplitude
                    * listener_slot.hearing_sensitivity;

                if effective_gain < INAUDIBLE_GAIN_THRESHOLD {
                    continue;
                }

                // Stereo spatial pan
                let pan = Self::calculate_spatial_panning(
                    listener_slot.position,
                    listener_slot.yaw,
                    emitter_slot.position,
                );

                // Deterministic P2P pairing token
                let p2p_token = (emitter_slot.entity_id.wrapping_mul(0x9E37_79B9_7F4A_7C15))
                    ^ (listener_slot.entity_id.wrapping_mul(0xBF58_476D_1CE4_E5B9));

                let descriptor = VoicePeerDescriptor {
                    speaker_id: emitter_slot.entity_id,
                    gain: effective_gain.clamp(0.0, 1.0),
                    pan,
                    low_pass_cutoff_hz: occlusion.low_pass_cutoff_hz,
                    occlusion_db: occlusion.occlusion_db,
                    distance_meters: dist,
                    p2p_token,
                };

                // In-place insertion sort into candidates array (top N loudest)
                Self::insert_channel_candidate(&mut candidates, &mut candidate_count, descriptor);
            }

            update.channels = candidates;
            update.channel_count = candidate_count;

            if let Some(target) = out_updates.get_mut(processed_listeners) {
                *target = update;
                processed_listeners += 1;
            }
        }

        processed_listeners
    }

    /// Inserts a descriptor into the fixed channels array in descending order of gain.
    fn insert_channel_candidate(
        channels: &mut [Option<VoicePeerDescriptor>; MAX_VOICE_CHANNELS_PER_LISTENER],
        count: &mut usize,
        desc: VoicePeerDescriptor,
    ) {
        let max_len = MAX_VOICE_CHANNELS_PER_LISTENER;
        let current_count = *count;

        // If array is full and new gain is smaller than the smallest, ignore
        if current_count >= max_len {
            if let Some(last) = channels[max_len - 1] {
                if desc.gain <= last.gain {
                    return;
                }
            }
        }

        // Find insertion point
        let mut insert_idx = current_count.min(max_len);
        for (i, item) in channels.iter().enumerate().take(current_count.min(max_len)) {
            if let Some(existing) = item {
                if desc.gain > existing.gain {
                    insert_idx = i;
                    break;
                }
            }
        }

        if insert_idx < max_len {
            // Shift elements right
            for i in (insert_idx + 1..max_len).rev() {
                channels[i] = channels[i - 1];
            }
            channels[insert_idx] = Some(desc);
            if *count < max_len {
                *count += 1;
            }
        }
    }
}
