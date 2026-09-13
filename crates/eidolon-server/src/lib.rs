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
pub mod memory_budget;
pub mod metrics;
pub mod multi_process;
pub mod platform_bench;
pub mod queue;
pub mod rolling;
pub mod soak;
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
pub use memory_budget::{
    audit_system_struct_sizes, PlayerMemoryBudget, StructMemoryProfile,
    MAX_PLAYER_MEMORY_CEILING_BYTES,
};
pub use metrics::{
    LatencyHistogram, PrometheusExporter, ServerMetrics, ZoneMetrics, DEFAULT_TICK_BUCKETS_MICROS,
};
pub use multi_process::{
    run_worker_if_requested, ProcessSupervisor, RealSocketPacket, RealSocketZoneNode,
    CLUSTER_SOCKET_MAGIC, OP_HEARTBEAT, OP_MIGRATE, OP_MIGRATE_ACK, OP_TRANSACTION,
};
pub use platform_bench::{BenchmarkMetric, PlatformBenchmarkReport, PlatformBenchmarkRunner};
pub use queue::{NetworkPacket, SpscPacketQueue};
pub use rolling::{RollingClusterNode, RollingUpgradeSimulator, UpgradeSession};
pub use soak::{SoakConfig, SoakTelemetry, SoakTestRunner};
pub use tick::{TickCoordinator, TickMetrics, DEFAULT_TICK_MICROS};
