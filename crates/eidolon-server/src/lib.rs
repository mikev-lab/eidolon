//! Headless server engine library for the eidolon MMO server architecture.
//!
//! Provides the 20 Hz authoritative simulation tick coordinator, decoupled UDP network I/O,
//! cross-thread bounded queues, and native Agones Kubernetes lifecycle integration.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod agones;
pub mod alerting;
pub mod clock;
pub mod config;
pub mod io;
pub mod metrics;
pub mod queue;
pub mod tick;

// Re-export primary types for ergonomic engine and integration test consumption.
pub use agones::{AgonesClient, AgonesState};
pub use alerting::{
    ActiveAlert, AlertConfig, AlertEvaluator, AlertKind, AlertSeverity, PodLifecycleController,
    PodLifecycleState,
};
pub use clock::{ClockGovernor, ClockGovernorConfig, ClockMetrics, TickPacingAction};
pub use config::ServerConfig;
pub use io::NetworkIoWorker;
pub use metrics::{
    LatencyHistogram, PrometheusExporter, ServerMetrics, ZoneMetrics, DEFAULT_TICK_BUCKETS_MICROS,
};
pub use queue::{NetworkPacket, SpscPacketQueue};
pub use tick::{TickCoordinator, TickMetrics, DEFAULT_TICK_MICROS};
