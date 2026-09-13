//! Hierarchical capacity admission control and graceful saturation degradation.
//!
//! Enforces strict priority-based packet shedding under bandwidth and CPU pressure:
//! Critical Combat Events > Nearby Movement (<10m) > Mid-Range Movement (<50m) > Far State > Cosmetics.

use crate::backpressure::BackpressureLevel;

/// Priority classification for outbound packets and state replication events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum PriorityClass {
    /// Priority 0 (Highest): Skill casts, damage, deaths, and authoritative state transitions.
    /// Never shed under backpressure.
    CriticalCombat = 0,
    /// Priority 1: High-frequency entity kinematics within 10 meters.
    ImmediateMovement = 1,
    /// Priority 2: Medium-frequency entity kinematics between 10 and 50 meters.
    MidRangeMovement = 2,
    /// Priority 3: Distant entity appearances, exits, and horizon state (>50m).
    FarState = 3,
    /// Priority 4 (Lowest): Visual attachments, particle triggers, and non-gameplay emotes.
    Cosmetic = 4,
}

impl PriorityClass {
    /// Returns true if this class represents critical gameplay that must not be shed.
    #[inline]
    pub const fn is_critical(self) -> bool {
        matches!(self, Self::CriticalCombat)
    }
}

/// Admission control metrics tracking admitted and shed packets per priority class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AdmissionMetrics {
    /// Admitted critical combat packets.
    pub admitted_critical: u64,
    /// Admitted immediate movement packets.
    pub admitted_immediate: u64,
    /// Admitted mid-range movement packets.
    pub admitted_mid_range: u64,
    /// Admitted far state packets.
    pub admitted_far_state: u64,
    /// Admitted cosmetic packets.
    pub admitted_cosmetic: u64,
    /// Shed cosmetic packets.
    pub shed_cosmetic: u64,
    /// Shed far state packets.
    pub shed_far_state: u64,
    /// Shed mid-range movement packets.
    pub shed_mid_range: u64,
    /// Shed immediate movement packets.
    pub shed_immediate: u64,
    /// Total bytes admitted in current epoch.
    pub total_bytes_admitted: u64,
}

/// Configurable parameters for the per-client admission controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionConfig {
    /// Maximum bytes allowed per simulation tick per client (e.g. 64 bytes for ~1.2 KB/s at 20 Hz).
    pub max_bytes_per_tick: usize,
    /// Maximum packet slots admitted per tick.
    pub max_packets_per_tick: usize,
    /// Reserved minimum bytes guaranteed exclusively for CriticalCombat traffic.
    pub reserved_critical_bytes: usize,
}

impl Default for AdmissionConfig {
    fn default() -> Self {
        Self {
            max_bytes_per_tick: 128,
            max_packets_per_tick: 8,
            reserved_critical_bytes: 32,
        }
    }
}

/// Per-client hierarchical admission controller enforcing byte budgets and priority shedding.
#[derive(Debug, Clone)]
pub struct AdmissionController {
    config: AdmissionConfig,
    current_tick: u64,
    bytes_used_this_tick: usize,
    packets_admitted_this_tick: usize,
    metrics: AdmissionMetrics,
}

impl AdmissionController {
    /// Creates a new admission controller with default configuration.
    pub fn new() -> Self {
        Self::with_config(AdmissionConfig::default())
    }

    /// Creates a new admission controller with custom configuration.
    pub fn with_config(config: AdmissionConfig) -> Self {
        Self {
            config,
            current_tick: 0,
            bytes_used_this_tick: 0,
            packets_admitted_this_tick: 0,
            metrics: AdmissionMetrics::default(),
        }
    }

    /// Advances the simulation tick, resetting per-tick byte and packet allowances.
    pub fn advance_tick(&mut self, tick: u64) {
        if self.current_tick != tick {
            self.current_tick = tick;
            self.bytes_used_this_tick = 0;
            self.packets_admitted_this_tick = 0;
        }
    }

    /// Returns the current simulation tick.
    #[inline]
    pub const fn current_tick(&self) -> u64 {
        self.current_tick
    }

    /// Returns a copy of the admission metrics.
    #[inline]
    pub const fn metrics(&self) -> AdmissionMetrics {
        self.metrics
    }

    /// Returns the remaining available byte budget for non-critical traffic this tick.
    pub fn remaining_general_budget(&self) -> usize {
        let general_ceiling = self
            .config
            .max_bytes_per_tick
            .saturating_sub(self.config.reserved_critical_bytes);
        general_ceiling.saturating_sub(self.bytes_used_this_tick)
    }

    /// Evaluates whether an outgoing packet should be admitted or shed based on
    /// priority hierarchy, active backpressure level, and per-tick budgets.
    pub fn try_admit(
        &mut self,
        priority: PriorityClass,
        byte_size: usize,
        pressure: BackpressureLevel,
    ) -> bool {
        // Step 1: Evaluate Backpressure Level Shedding Policy
        match pressure {
            BackpressureLevel::Normal => {
                // All classes eligible, subject to budget limits
            }
            BackpressureLevel::Elevated => {
                // Shed cosmetics under elevated pressure
                if priority == PriorityClass::Cosmetic {
                    self.metrics.shed_cosmetic += 1;
                    return false;
                }
            }
            BackpressureLevel::Saturated => {
                // Shed cosmetics and far state
                if priority == PriorityClass::Cosmetic {
                    self.metrics.shed_cosmetic += 1;
                    return false;
                }
                if priority == PriorityClass::FarState {
                    self.metrics.shed_far_state += 1;
                    return false;
                }
                // Decimate mid-range movement (50% decimation: admit only on even ticks)
                if priority == PriorityClass::MidRangeMovement
                    && !self.current_tick.is_multiple_of(2)
                {
                    self.metrics.shed_mid_range += 1;
                    return false;
                }
            }
            BackpressureLevel::Critical => {
                // Under critical pressure, drop everything non-immediate
                if priority == PriorityClass::Cosmetic {
                    self.metrics.shed_cosmetic += 1;
                    return false;
                }
                if priority == PriorityClass::FarState {
                    self.metrics.shed_far_state += 1;
                    return false;
                }
                if priority == PriorityClass::MidRangeMovement {
                    self.metrics.shed_mid_range += 1;
                    return false;
                }
                // Decimate immediate movement (admit only every 2nd tick at 10 Hz)
                if priority == PriorityClass::ImmediateMovement
                    && !self.current_tick.is_multiple_of(2)
                {
                    self.metrics.shed_immediate += 1;
                    return false;
                }
            }
        }

        // Step 2: Critical Combat bypasses general budget constraints
        if priority.is_critical() {
            self.bytes_used_this_tick += byte_size;
            self.packets_admitted_this_tick += 1;
            self.metrics.admitted_critical += 1;
            self.metrics.total_bytes_admitted += byte_size as u64;
            return true;
        }

        // Step 3: Check packet slot count limit
        if self.packets_admitted_this_tick >= self.config.max_packets_per_tick {
            self.record_shed(priority);
            return false;
        }

        // Step 4: Check byte budget limits
        let general_limit = self
            .config
            .max_bytes_per_tick
            .saturating_sub(self.config.reserved_critical_bytes);

        if self.bytes_used_this_tick.saturating_add(byte_size) > general_limit {
            self.record_shed(priority);
            return false;
        }

        // Admit packet
        self.bytes_used_this_tick += byte_size;
        self.packets_admitted_this_tick += 1;
        self.metrics.total_bytes_admitted += byte_size as u64;

        match priority {
            PriorityClass::CriticalCombat => {}
            PriorityClass::ImmediateMovement => self.metrics.admitted_immediate += 1,
            PriorityClass::MidRangeMovement => self.metrics.admitted_mid_range += 1,
            PriorityClass::FarState => self.metrics.admitted_far_state += 1,
            PriorityClass::Cosmetic => self.metrics.admitted_cosmetic += 1,
        }

        true
    }

    fn record_shed(&mut self, priority: PriorityClass) {
        match priority {
            PriorityClass::CriticalCombat => {}
            PriorityClass::ImmediateMovement => self.metrics.shed_immediate += 1,
            PriorityClass::MidRangeMovement => self.metrics.shed_mid_range += 1,
            PriorityClass::FarState => self.metrics.shed_far_state += 1,
            PriorityClass::Cosmetic => self.metrics.shed_cosmetic += 1,
        }
    }
}

impl Default for AdmissionController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_hierarchy_under_backpressure() {
        let mut controller = AdmissionController::with_config(AdmissionConfig {
            max_bytes_per_tick: 500,
            max_packets_per_tick: 20,
            reserved_critical_bytes: 50,
        });

        controller.advance_tick(1);

        // 1. Normal pressure admits all priorities
        assert!(controller.try_admit(PriorityClass::Cosmetic, 10, BackpressureLevel::Normal));
        assert!(controller.try_admit(PriorityClass::FarState, 10, BackpressureLevel::Normal));
        assert!(controller.try_admit(
            PriorityClass::MidRangeMovement,
            10,
            BackpressureLevel::Normal
        ));
        assert!(controller.try_admit(
            PriorityClass::ImmediateMovement,
            10,
            BackpressureLevel::Normal
        ));
        assert!(controller.try_admit(PriorityClass::CriticalCombat, 10, BackpressureLevel::Normal));

        // 2. Elevated pressure sheds cosmetic, admits higher priorities
        assert!(!controller.try_admit(PriorityClass::Cosmetic, 10, BackpressureLevel::Elevated));
        assert!(controller.try_admit(PriorityClass::FarState, 10, BackpressureLevel::Elevated));
        assert!(controller.try_admit(
            PriorityClass::CriticalCombat,
            10,
            BackpressureLevel::Elevated
        ));

        // 3. Saturated pressure sheds cosmetic and far state
        assert!(!controller.try_admit(PriorityClass::Cosmetic, 10, BackpressureLevel::Saturated));
        assert!(!controller.try_admit(PriorityClass::FarState, 10, BackpressureLevel::Saturated));

        // 4. Critical pressure sheds all non-immediate, preserves CriticalCombat
        assert!(!controller.try_admit(PriorityClass::Cosmetic, 10, BackpressureLevel::Critical));
        assert!(!controller.try_admit(PriorityClass::FarState, 10, BackpressureLevel::Critical));
        assert!(!controller.try_admit(
            PriorityClass::MidRangeMovement,
            10,
            BackpressureLevel::Critical
        ));
        assert!(controller.try_admit(
            PriorityClass::CriticalCombat,
            10,
            BackpressureLevel::Critical
        ));
    }

    #[test]
    fn test_frequency_decimation_under_saturation() {
        let mut controller = AdmissionController::with_config(AdmissionConfig {
            max_bytes_per_tick: 1000,
            max_packets_per_tick: 50,
            reserved_critical_bytes: 50,
        });

        // Tick 1 (odd): MidRangeMovement is shed under Saturated
        controller.advance_tick(1);
        assert!(!controller.try_admit(
            PriorityClass::MidRangeMovement,
            10,
            BackpressureLevel::Saturated
        ));

        // Tick 2 (even): MidRangeMovement is admitted under Saturated
        controller.advance_tick(2);
        assert!(controller.try_admit(
            PriorityClass::MidRangeMovement,
            10,
            BackpressureLevel::Saturated
        ));
    }

    #[test]
    fn test_budget_exhaustion_sheds_lower_priorities_preserves_critical() {
        let mut controller = AdmissionController::with_config(AdmissionConfig {
            max_bytes_per_tick: 100,
            max_packets_per_tick: 5,
            reserved_critical_bytes: 30, // general budget = 70 bytes
        });

        controller.advance_tick(1);

        // Admit 70 bytes of movement
        assert!(controller.try_admit(
            PriorityClass::ImmediateMovement,
            70,
            BackpressureLevel::Normal
        ));

        // Attempting another 10 bytes of cosmetic or far state exceeds general budget -> shed
        assert!(!controller.try_admit(PriorityClass::Cosmetic, 10, BackpressureLevel::Normal));
        assert!(!controller.try_admit(PriorityClass::FarState, 10, BackpressureLevel::Normal));

        // Critical combat bypasses general budget into its reserved allowance
        assert!(controller.try_admit(PriorityClass::CriticalCombat, 25, BackpressureLevel::Normal));
    }
}
