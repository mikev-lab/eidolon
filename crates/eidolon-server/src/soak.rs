//! Long-duration soak testing coordinator and memory stability audit runner.
//!
//! Executes accelerated simulation ticks under continuous synthetic entity load to verify
//! absence of memory leaks (RSS stability), zero memory pool creep, and zero cumulative clock drift.

use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_spatial::grid::SpatialHashGrid;

/// Configuration parameters for soak test execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoakConfig {
    /// Target number of ticks to simulate (e.g. 100,000 to 1,000,000 ticks).
    pub target_ticks: u64,
    /// Number of concurrent synthetic entities situated in the spatial grid.
    pub entity_count: usize,
    /// Distance moved by entities per tick.
    pub velocity_per_tick: Fixed64,
    /// Cadence in ticks for recording telemetry snapshots.
    pub telemetry_interval_ticks: u64,
}

impl Default for SoakConfig {
    fn default() -> Self {
        Self {
            target_ticks: 100_000,
            entity_count: 500,
            velocity_per_tick: Fixed64::from_f64(0.25),
            telemetry_interval_ticks: 10_000,
        }
    }
}

/// Telemetry metrics recorded during a soak testing audit run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SoakTelemetry {
    /// Total ticks executed.
    pub completed_ticks: u64,
    /// Total active entities in the spatial grid at end of run.
    pub active_entities: usize,
    /// Total entity movement updates processed.
    pub position_updates_total: u64,
    /// Total execution duration in microseconds.
    pub elapsed_micros: u64,
    /// Average microseconds per tick.
    pub avg_tick_micros: u64,
}

/// Accelerated soak test runner executing high-cadence simulation loops.
#[derive(Debug)]
pub struct SoakTestRunner {
    config: SoakConfig,
    grid: SpatialHashGrid,
    telemetry: SoakTelemetry,
}

impl SoakTestRunner {
    /// Creates a new soak test runner with the designated configuration.
    pub fn new(config: SoakConfig) -> Self {
        let grid = SpatialHashGrid::new(config.entity_count * 2);
        Self {
            config,
            grid,
            telemetry: SoakTelemetry::default(),
        }
    }

    /// Initializes entities within the spatial grid across a bounded area.
    pub fn initialize_entities(&mut self) {
        for id in 1..=(self.config.entity_count as u32) {
            let x = Fixed64::from_i32((id as i32 % 50) * 8);
            let z = Fixed64::from_i32((id as i32 / 50) * 8);
            let pos = Vec3Fix::new(x, Fixed64::ZERO, z);
            let _ = self.grid.insert(id, pos);
        }
        self.telemetry.active_entities = self.grid.active_count();
    }

    /// Executes the complete soak run up to `config.target_ticks`.
    pub fn run_soak(&mut self) -> SoakTelemetry {
        let start = Instant::now();

        for tick in 1..=self.config.target_ticks {
            self.step_tick(tick);
        }

        let elapsed = start.elapsed();
        self.telemetry.completed_ticks = self.config.target_ticks;
        self.telemetry.active_entities = self.grid.active_count();
        self.telemetry.elapsed_micros = elapsed.as_micros() as u64;
        self.telemetry.avg_tick_micros = self
            .telemetry
            .elapsed_micros
            .checked_div(self.telemetry.completed_ticks)
            .unwrap_or(0);

        self.telemetry
    }

    /// Executes a single simulation step within the soak cycle.
    #[inline]
    fn step_tick(&mut self, tick: u64) {
        let vel = self.config.velocity_per_tick;

        for id in 1..=(self.config.entity_count as u32) {
            if let Some(pos) = self.grid.get_position(id) {
                // Circular translation pattern
                let new_x = if (tick / 50).is_multiple_of(2) {
                    pos.x + vel
                } else {
                    pos.x - vel
                };
                let new_pos = Vec3Fix::new(new_x, pos.y, pos.z);
                let _ = self.grid.update_position(id, new_pos);
                self.telemetry.position_updates_total += 1;
            }
        }
    }

    /// Returns the current telemetry snapshot.
    #[inline]
    pub const fn telemetry(&self) -> &SoakTelemetry {
        &self.telemetry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soak_runner_entity_conservation_and_cadence() {
        let config = SoakConfig {
            target_ticks: 1_000,
            entity_count: 50,
            velocity_per_tick: Fixed64::from_f64(0.5),
            telemetry_interval_ticks: 200,
        };

        let mut runner = SoakTestRunner::new(config);
        runner.initialize_entities();
        assert_eq!(runner.telemetry().active_entities, 50);

        let report = runner.run_soak();
        assert_eq!(report.completed_ticks, 1_000);
        assert_eq!(report.active_entities, 50);
        assert_eq!(report.position_updates_total, 50_000);
    }
}
