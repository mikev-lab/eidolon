//! Authoritative In-Memory Black-Box Flight Recorder and Replay Session Coordinator.
//!
//! Maintains rolling circular buffers of client input frames and authoritative
//! keyframe snapshots, enabling instant time-travel debugging, backward/forward scrubbing,
//! variable-speed replay, and referee divergence detection with zero dynamic allocations.

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::quant::QuantizedYaw;
use eidolon_core::replay::{
    compute_entities_checksum, extrapolate_with_input, EntityStateSnapshot, KeyframeSnapshot,
    PlaybackSpeed, RecordedInputFrame, ReplayDivergence, MAX_KEYFRAME_ENTITIES,
};

/// Maximum client input frames retained in rolling memory.
pub const MAX_INPUT_FRAMES_CAPACITY: usize = 256;
/// Maximum keyframe snapshots retained in rolling memory.
pub const MAX_KEYFRAMES_CAPACITY: usize = 32;

/// Errors arising during replay operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayError {
    /// Requested tick is outside the available recording buffer window.
    TickOutOfRange {
        /// Requested tick.
        requested: u64,
        /// Earliest available recorded tick.
        earliest: u64,
        /// Latest available recorded tick.
        latest: u64,
    },
    /// No valid keyframe found preceding the target tick.
    KeyframeNotFound,
}

impl core::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TickOutOfRange {
                requested,
                earliest,
                latest,
            } => write!(
                f,
                "Requested tick {requested} out of range [{earliest}, {latest}]"
            ),
            Self::KeyframeNotFound => write!(f, "No preceding keyframe found for target tick"),
        }
    }
}

impl std::error::Error for ReplayError {}

/// Rolling flight recorder capturing compact input frames and periodic state keyframes.
#[derive(Debug, Clone)]
pub struct FlightRecorder {
    input_frames: [Option<RecordedInputFrame>; MAX_INPUT_FRAMES_CAPACITY],
    input_head: usize,
    input_count: usize,
    keyframes: [Option<KeyframeSnapshot>; MAX_KEYFRAMES_CAPACITY],
    keyframe_head: usize,
    keyframe_count: usize,
    keyframe_interval_ticks: u32,
    latest_tick: u64,
    earliest_tick: u64,
}

impl Default for FlightRecorder {
    fn default() -> Self {
        Self::new(20) // Default 20 ticks = 1.0s at 20 Hz
    }
}

impl FlightRecorder {
    /// Creates a new flight recorder with specified keyframe interval.
    pub fn new(keyframe_interval_ticks: u32) -> Self {
        Self {
            input_frames: [const { None }; MAX_INPUT_FRAMES_CAPACITY],
            input_head: 0,
            input_count: 0,
            keyframes: [const { None }; MAX_KEYFRAMES_CAPACITY],
            keyframe_head: 0,
            keyframe_count: 0,
            keyframe_interval_ticks: keyframe_interval_ticks.max(1),
            latest_tick: 0,
            earliest_tick: 0,
        }
    }

    /// Records an incoming client input frame into the rolling ring buffer.
    pub fn record_input(
        &mut self,
        tick: u64,
        entity_id: u32,
        velocity: Vec3Fix,
        yaw: QuantizedYaw,
        flags: u8,
    ) {
        let frame = RecordedInputFrame {
            tick,
            entity_id,
            velocity,
            yaw,
            flags,
        };

        if (self.input_count == 0 && self.keyframe_count == 0) || tick < self.earliest_tick {
            self.earliest_tick = tick;
        }
        if tick > self.latest_tick {
            self.latest_tick = tick;
        }

        self.input_frames[self.input_head] = Some(frame);
        self.input_head = (self.input_head + 1) % MAX_INPUT_FRAMES_CAPACITY;
        if self.input_count < MAX_INPUT_FRAMES_CAPACITY {
            self.input_count += 1;
        }
    }

    /// Records an authoritative full-state keyframe snapshot.
    pub fn record_keyframe(&mut self, tick: u64, entities: &[EntityStateSnapshot]) {
        let snapshot = KeyframeSnapshot::new(tick, entities);

        if (self.input_count == 0 && self.keyframe_count == 0) || tick < self.earliest_tick {
            self.earliest_tick = tick;
        }
        if tick > self.latest_tick {
            self.latest_tick = tick;
        }

        self.keyframes[self.keyframe_head] = Some(snapshot);
        self.keyframe_head = (self.keyframe_head + 1) % MAX_KEYFRAMES_CAPACITY;
        if self.keyframe_count < MAX_KEYFRAMES_CAPACITY {
            self.keyframe_count += 1;
        }
    }

    /// Returns the latest recorded server tick.
    #[inline]
    pub fn latest_tick(&self) -> u64 {
        self.latest_tick
    }

    /// Returns the earliest recorded server tick currently residing in memory.
    #[inline]
    pub fn earliest_tick(&self) -> u64 {
        self.earliest_tick
    }

    /// Returns the configured keyframe interval in server ticks.
    #[inline]
    pub fn keyframe_interval(&self) -> u32 {
        self.keyframe_interval_ticks
    }

    /// Finds the nearest recorded keyframe whose tick is less than or equal to `target_tick`.
    pub fn find_nearest_prior_keyframe(&self, target_tick: u64) -> Option<&KeyframeSnapshot> {
        let mut best: Option<&KeyframeSnapshot> = None;

        for kf in self.keyframes.iter().take(self.keyframe_count).flatten() {
            if kf.tick <= target_tick {
                if let Some(current_best) = best {
                    if kf.tick > current_best.tick {
                        best = Some(kf);
                    }
                } else {
                    best = Some(kf);
                }
            }
        }

        best
    }

    /// Copies recorded input frames within `[start_tick, end_tick]` into caller-provided buffer.
    pub fn copy_input_frames_range(
        &self,
        start_tick: u64,
        end_tick: u64,
        out: &mut [RecordedInputFrame],
    ) -> usize {
        let mut count = 0;
        for frame in self.input_frames.iter().take(self.input_count).flatten() {
            if frame.tick >= start_tick && frame.tick <= end_tick && count < out.len() {
                out[count] = *frame;
                count += 1;
            }
        }
        count
    }

    /// Instantiates an active replay session initialized to the earliest recorded keyframe.
    pub fn create_replay_session(&self) -> Result<ReplaySession<'_>, ReplayError> {
        if self.keyframe_count == 0 {
            return Err(ReplayError::KeyframeNotFound);
        }

        let initial_kf = self
            .find_nearest_prior_keyframe(self.earliest_tick)
            .ok_or(ReplayError::KeyframeNotFound)?;

        Ok(ReplaySession {
            current_tick: initial_kf.tick,
            speed: PlaybackSpeed::Normal,
            active_entities: initial_kf.entities,
            entity_count: initial_kf.entity_count,
            recorder: self,
        })
    }
}

/// Active replay session supporting time-travel scrubbing, playback control, and divergence audit.
pub struct ReplaySession<'a> {
    current_tick: u64,
    speed: PlaybackSpeed,
    active_entities: [Option<EntityStateSnapshot>; MAX_KEYFRAME_ENTITIES],
    entity_count: usize,
    recorder: &'a FlightRecorder,
}

impl<'a> ReplaySession<'a> {
    /// Returns the current playback tick.
    #[inline]
    pub fn current_tick(&self) -> u64 {
        self.current_tick
    }

    /// Returns the current playback speed multiplier.
    #[inline]
    pub fn playback_speed(&self) -> PlaybackSpeed {
        self.speed
    }

    /// Sets the playback speed.
    pub fn set_playback_speed(&mut self, speed: PlaybackSpeed) {
        self.speed = speed;
    }

    /// Computes the active state checksum at the current replay tick.
    pub fn current_checksum(&self) -> u32 {
        compute_entities_checksum(&self.active_entities[..self.entity_count])
    }

    /// Retrieves an entity's extrapolated state at the current replay tick.
    pub fn get_entity_state(&self, entity_id: u32) -> Option<EntityStateSnapshot> {
        for ent in self
            .active_entities
            .iter()
            .take(self.entity_count)
            .flatten()
        {
            if ent.entity_id == entity_id {
                return Some(*ent);
            }
        }
        None
    }

    /// Scrubs forward or backward to an exact target server tick.
    ///
    /// Locates the nearest preceding keyframe, restores its authoritative state,
    /// and forward-replays all input frames up to `target_tick` with 100% determinism.
    pub fn scrub_to_tick(&mut self, target_tick: u64) -> Result<u64, ReplayError> {
        let earliest = self.recorder.earliest_tick();
        let latest = self.recorder.latest_tick();

        if target_tick < earliest || target_tick > latest {
            return Err(ReplayError::TickOutOfRange {
                requested: target_tick,
                earliest,
                latest,
            });
        }

        let base_kf = self
            .recorder
            .find_nearest_prior_keyframe(target_tick)
            .ok_or(ReplayError::KeyframeNotFound)?;

        // Restore keyframe state
        self.current_tick = base_kf.tick;
        self.active_entities = base_kf.entities;
        self.entity_count = base_kf.entity_count;

        // Forward-replay inputs from base_kf.tick + 1 up to target_tick
        let dt = Fixed64::from_f64(0.050); // 50ms per tick
        let mut frame_buf = [RecordedInputFrame::default(); 64];

        for t in (base_kf.tick + 1)..=target_tick {
            let count = self.recorder.copy_input_frames_range(t, t, &mut frame_buf);
            for frame in frame_buf.iter().take(count) {
                // Apply input frame to matching entity
                for ent in self
                    .active_entities
                    .iter_mut()
                    .take(self.entity_count)
                    .flatten()
                {
                    if ent.entity_id == frame.entity_id {
                        *ent = extrapolate_with_input(ent, frame, dt);
                    }
                }
            }
            self.current_tick = t;
        }

        Ok(self.current_tick)
    }

    /// Steps forward by `delta_ticks` bounded by the latest recorded tick.
    pub fn step_forward(&mut self, delta_ticks: u64) -> Result<u64, ReplayError> {
        let target = (self.current_tick + delta_ticks).min(self.recorder.latest_tick());
        self.scrub_to_tick(target)
    }

    /// Steps backward by `delta_ticks` bounded by the earliest recorded tick.
    pub fn step_backward(&mut self, delta_ticks: u64) -> Result<u64, ReplayError> {
        let target = self
            .current_tick
            .saturating_sub(delta_ticks)
            .max(self.recorder.earliest_tick());
        self.scrub_to_tick(target)
    }

    /// Compares live server tick state against recorded replay state and identifies divergence.
    ///
    /// Returns `None` if states are in bit-for-bit parity; returns `Some(ReplayDivergence)`
    /// containing the divergent entity and drift distance on mismatch.
    pub fn detect_divergence(
        &self,
        live_tick: u64,
        live_checksum: u32,
        live_entities: &[EntityStateSnapshot],
    ) -> Option<ReplayDivergence> {
        if live_tick != self.current_tick {
            return None;
        }

        let recorded_checksum = self.current_checksum();
        if recorded_checksum == live_checksum {
            return None;
        }

        // Identify which entity diverged
        let mut divergent_entity = None;
        let mut max_drift = 0.0f64;

        for live_ent in live_entities {
            if let Some(recorded_ent) = self.get_entity_state(live_ent.entity_id) {
                let dist_sq = live_ent.position.distance_squared(recorded_ent.position);
                let dist = dist_sq.to_f64().sqrt();
                if dist > 0.001 && dist > max_drift {
                    max_drift = dist;
                    divergent_entity = Some(live_ent.entity_id);
                }
            }
        }

        Some(ReplayDivergence {
            tick: live_tick,
            expected_checksum: recorded_checksum,
            actual_checksum: live_checksum,
            divergent_entity_id: divergent_entity,
            drift_distance: max_drift,
        })
    }
}
