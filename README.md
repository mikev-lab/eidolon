<div align="center">

# eidolon

**High-concurrency, ultra-low-bandwidth (<1 KB/s) zoned & instanced MMO world server engine in Rust.**

[![License: BSL 1.1](https://img.shields.io/badge/License-BSL_1.1_(Fair_Source)-blue.svg)](./LICENSE)
[![Indie Grant: <$1M Free](https://img.shields.io/badge/Indie_Grant-%3C$1M_Free-success.svg)](./LICENSE)
[![Language: Rust](https://img.shields.io/badge/Language-Rust_2021-orange.svg)](https://www.rust-lang.org/)
[![Dependencies: 0 External](https://img.shields.io/badge/Dependencies-0_External_Runtime_Crates-brightgreen.svg)](#first-principles-zero-dependency-architecture)
[![Wire Footprint: <1.2 KB/s](https://img.shields.io/badge/Wire_Budget-1.02_KB%2Fs_Verified-blueviolet.svg)](#the-engineering-problem-the-mmo-egress-trap)
[![Safety: Deny Unsafe](https://img.shields.io/badge/Safety-100%25_Safe_Rust-success.svg)](#engineering-governance--safety)

<p align="center">
  <a href="#overview">Overview</a> •
  <a href="#the-engineering-problem-the-mmo-egress-trap">The Egress Trap</a> •
  <a href="#key-architectural-pillars">Architectural Pillars</a> •
  <a href="#world-topology">World Topology</a> •
  <a href="#performance-benchmarks--wire-metrics">Benchmarks</a> •
  <a href="#workspace-crates">Workspace Crates</a> •
  <a href="#quickstart--developer-guide">Quickstart</a> •
  <a href="#technical-documentation">Documentation</a> •
  <a href="#licensing--fair-source-model">License</a>
</p>

</div>

---

## Overview

**eidolon** is an authoritative, high-concurrency headless MMO server engine engineered from first principles in pure Rust. It is built to resolve the two most difficult economic and operational challenges in multiplayer online games: **crippling cloud network egress bills** and the **"End-of-Service" (EoS) cliff**.

Most multiplayer backend architectures force a painful tradeoff:
1. **Matchmaking lobby frameworks** (e.g. 5v5 session runners) that excel at short, ephemeral matches but lack the capability to host contiguous, persistent open worlds.
2. **Monolithic legacy MMO servers** that require massive always-on cloud footprints, rely on opaque third-party middleware, and suffer from garbage collection (GC) latency spikes under heavy player density.

`eidolon` solves both problems through an architectural mandate: **host for virtually $0/month at small scale (fitting within Google Cloud's free `e2-micro` tier), while possessing the deterministic efficiency to scale horizontally to millions of concurrent users on Kubernetes with Agones without an architectural rewrite.**

---

## The Engineering Problem: The MMO Egress Trap

In large-scale multiplayer games, **network egress (not CPU compute) is the dominant operational expense.** Standard cloud providers (AWS, GCP, Azure) bill public internet data egress between **$0.05 and $0.12 per gigabyte**.

An unoptimized MMO streaming raw 32-bit coordinates at 20 KB/s per player to 100,000 concurrent users (CCU) burns over **$237,000 per month** ($2.85M/year) on bandwidth alone. At 1,000,000 CCU, egress expenses exceed **$2.37M per month** ($28.5M/year).

`eidolon` aggressively compresses the authoritative client wire footprint down to an average of **1.02 KB/s (L3/L4 wire)**, cutting bandwidth consumption by **94.9%**:

| Scale / Concurrency | Unoptimized Baseline (20 KB/s) | Semi-Optimized (8 KB/s) | **eidolon Authoritative Wire (1.02 KB/s)** | Annual Studio Savings |
| :--- | :--- | :--- | :--- | :--- |
| **Bandwidth per Player** | 20.0 KB/s (160 kbps) | 8.0 KB/s (64 kbps) | **1.02 KB/s (8.2 kbps)** | **94.9% Bandwidth Reduction** |
| **5 Players (Dev / EoS)** | $11.87 / mo ($142 / yr) | $4.75 / mo ($57 / yr) | **$0.61 / mo (100% Free on GCP)** | **$135 / yr (Free Tier)** |
| **1,000 CCU (Indie / Private)** | $2,374 / mo ($28.5k / yr) | $949 / mo ($11.4k / yr) | **$121 / mo ($1.45k / yr)** | **$27,036 / year** |
| **10,000 CCU (Mid-Scale MMO)** | $23,738 / mo ($285k / yr) | $9,495 / mo ($114k / yr) | **$1,211 / mo ($14.5k / yr)** | **$270,324 / year** |
| **100,000 CCU (Top Steam Title)**| $237,381 / mo ($2.85M / yr) | $94,952 / mo ($1.14M / yr)| **$12,106 / mo ($145k / yr)** | **$2,703,300 / year** |
| **1,000,000 CCU (Global Hit)** | $2,373,811 / mo ($28.5M / yr)| $949,524 / mo ($11.4M / yr)| **$121,064 / mo ($1.45M / yr)** | **$27,032,964 / year** |

*Calculated at blended $0.07/GB cloud internet egress across 16 active gameplay hours per player day. See [`docs/COST_ANALYSIS.md`](./docs/COST_ANALYSIS.md) for full financial breakdowns.*

---

## Key Architectural Pillars

### 1. Sub-1KB/s Authoritative Wire Budget
- **Intent-Based Dead Reckoning:** Transmits velocity and movement intent only on acceleration, heading shift, or state transition. Server and client execute deterministic extrapolation between updates, eliminating up to **85% of movement packets**.
- **Dynamic 3-Tier Area of Interest (AoI):**
  - *Immediate Tier (< 10m):* High-frequency updates at **10 Hz** (combat targets, close interactions).
  - *Mid Tier (10m - 50m):* Interpolated updates at **2 Hz** (patrolling NPCs, ambient players).
  - *Horizon Tier (> 50m):* Event-only state changes (visibility enter/leave).
- **Asymmetric Coordinate Quantization:** Local cell coordinates quantized to 16-bit integers ($X, Z$) with sub-millimeter precision ($\approx 0.97\text{ mm}$), 12-bit elevation ($Y \approx 7.8\text{ mm}$), and 1-byte yaw ($256$ discrete angles $\approx 1.4^\circ$).
- **7-Byte Wire Bitpacking:** Full spatial transform, heading, and action bitflags packed into just **7 bytes**.

### 2. Deterministic Zero-GC Simulation Tick Loop
- **Fixed 20 Hz / 50ms Cadence:** Synchronous simulation runner with target-instant monotonic clock pacing.
- **Zero Allocations in Hot Paths:** Zero runtime heap allocations (`Vec::new()`, `Box`, `String`) inside the simulation loop. Entity slots, spatial buckets, and packet queues use pre-allocated contiguous memory pools.
- **Microsecond Execution Timing:** Tick budget enforcement with phase instrumentation (Ingress, Simulation, Spatial Partitioning, Egress Flush). Median tick duration is **3.06 ms**, leaving **>93% CPU headroom**.

### 3. Physical Durability & RPO = 0 Commit Semantics
- **Synchronous Disk Barrier:** Physical POSIX `fdatasync` (`sync_data`) executes before the server acknowledges economic mutations (currency spends, item trades, gacha pulls) to the client, guaranteeing **Recovery Point Objective (RPO) = 0** across process crashes or host failure.
- **Crash Recovery & Truncated Write Defense:** The append-only durable journal (`DurableFileJournal`) employs 8-byte framing magic (`0xE1D0_4A52`) and Adler-32 checksums. Incomplete trailing writes from sudden `SIGKILL` or power loss are cleanly discarded during recovery without panics or in-memory corruption.
- **Disposable Zone Server Reconstruction:** A replacement zone node reconstructs authoritative world state from a base checkpoint plus sequential WAL replay in **under 2.0 seconds (RTO < 2.0s)**. Full specifications in [`docs/DISASTER_RECOVERY.md`](./docs/DISASTER_RECOVERY.md).

### 4. Distributed Session Security, Anti-DoS & Identity Pipeline
- **3-Tier Identity Hierarchy:** Enforces an `AccountId` -> `SessionTicket` -> `CharacterId` capability pipeline.
- **Concurrent Login Revocation:** Monotonic generation fences immediately invalidate prior session capability tokens when a user logs in from a new device, eliminating dual-login duplication exploits.
- **Cross-Account Write Protection:** Cryptographically fences character mutations, rejecting unauthorized cross-account actions.
- **Cryptographic Association:** Ephemeral 3-way handshake (`ConnectChallenge` / `ConnectFinalize`) authenticated via native HMAC-SHA256 with 64-bit sequence replay windows.
- **Anti-DoS Quotas:** Per-socket token bucket rate policers (40 pps / 32 KB/s) and a hard 64 KB per-connection memory ceiling.

### 5. Unified End-to-End Pipeline Backpressure
- **Bounded Queues Throughout:** Hard compile-time capacities on ingress, simulation, AoI, and egress queues.
- **Hierarchical Priority Shedding:** Under extreme network backpressure, low-priority packets are shed systematically while preserving critical state:
  $$\text{Critical Combat} > \text{Nearby Movement (<10m)} > \text{Mid-Range (<50m)} > \text{Far State} > \text{Cosmetics}$$
- **Ingress & Egress Saturation Defense:** Stale movement updates are dropped under saturation while 100% reliable command delivery is guaranteed.

### 6. First-Principles Native Cryptography (Zero Dependencies)
- **Standardized Implementations:** Native, first-principles SHA-256 and HMAC-SHA256 verified bit-for-bit against official NIST CAVP vectors and all 7 RFC 4231 standard test cases.
- **Side-Channel Defense:** Constant-time 32-byte equality check (`constant_time_eq`) protected by compiler `core::hint::black_box` barriers to defeat timing side-channel attacks.
- **Enterprise Pluggability:** Abstract `CryptoProvider` trait enables drop-in substitution of hardware-accelerated or FIPS-140 certified HSM backends.

### 7. Gacha & Long-Tail EoS Preservation
- **Scale-to-Zero Co-op Instances:** Ephemeral battle rooms spin up in `<50ms` only when parties enter a portal. When no instances are running, compute cost drops to $0.
- **Cold-State Account Hibernation:** Inactive player rosters, pity counters, and inventories compress into cold storage binary snapshots (<256 bytes per account, <2 µs hydration), enabling games to run in **Perpetual Maintenance Mode** indefinitely.

---

## World Topology

`eidolon` uses a hybrid zoned and instanced topology:

```text
                              ┌───────────────────────────────┐
                              │     eidolon-server Binary     │
                              │    (Multi-Threaded Runner)    │
                              └───────────────┬───────────────┘
                                              │
                      ┌───────────────────────┴───────────────────────┐
                      ▼                                               ▼
      ┌───────────────────────────────┐               ┌───────────────────────────────┐
      │     Persistent Open World     │               │   Ephemeral Dungeon Instances │
      │        (Spatial Grid)         │               │     (Isolated Room Threads)   │
      ├───────────────────────────────┤               ├───────────────────────────────┤
      │ • Seamless Zone A             │               │ • Ephemeral Dungeon Room #1   │
      │ • Seamless Zone B             │               │ • Co-op Battle Instance #402  │
      │ • In-memory entity migration  │               │ • Dynamic thread allocation   │
      │ • Zero loading screens        │               │ • Reclaim memory on exit      │
      └───────────────────────────────┘               └───────────────────────────────┘
```

* **Seamless Persistent Zones:** Open-world zones are partitioned into flat spatial hash grids. When an entity crosses a boundary, ownership is handed off in memory across spatial grids with 16-meter overlapping seams, eliminating loading screens.
* **Ephemeral Dungeon Instances:** Dungeons, raids, arenas, and housing interiors are lightweight private rooms allocated dynamically on party entry and reclaimed immediately on exit.

---

## Performance Benchmarks & Wire Metrics

All performance claims and wire budgets are measured empirically using automated integration test suites and high-precision microbenchmark harnesses.

### 1. Verified Wire Footprint (2,000 Concurrent Bots, 100 Observers)

From the automated load simulation harness (`crates/eidolon-server/tests/simulation_harness.rs`) simulating 2,000 active bots in a multi-zone world topology with 100 active observer clients receiving full 3D AoI queries, visibility reconciliation, and tiered replication across 4 UDP multiplexed sockets:

| Metric | Budget Target | Measured Production Result | Status |
| :--- | :--- | :--- | :--- |
| **Server Replication Egress (L7 Payload)** | < 1,228.8 B/s (1.2 KB/s) | **766.51 B/s (0.75 KB/s)** | **37.6% Headroom** |
| **Server Replication Egress (L3/L4 Wire)** | < 1,228.8 B/s (1.2 KB/s) | **1,046.51 B/s (1.02 KB/s)** | **Conforms to Wire Budget (Includes 28B IP/UDP)** |
| **Client Input Ingress (L7 Payload)** | < 1,228.8 B/s (1.2 KB/s) | **15.43 B/s (0.02 KB/s)** | **98.7% Under Budget (Dead Reckoning)** |
| **Client Input Ingress (L3/L4 Wire)** | < 1,228.8 B/s (1.2 KB/s) | **30.32 B/s (0.03 KB/s)** | **97.5% Under Budget (Includes 28B IP/UDP)** |
| **Reconstructible Replication Accuracy** | < 2.0 mm tolerance | **0.977 mm (< 1.0 mm)** | **Sub-millimeter Global Reconstruction** |
| **Authoritative Tick Cadence** | 20.0 Hz (50.0 ms) | **20.0 Hz (50.0 ms +/- 5.0 ms)** | **Locked (Target-Instant Pacing)** |
| **In-Memory Seam Migrations** | 100% Retained | **1,580 transitions / 0 lost entities** | **100% Zero-Loss Accounting** |
| **Tick Load Shedding Level** | Level 0 (Normal) | **Level 0 (Zero Overruns)** | **100% Simulation Headroom** |
| **Memory Leak Audit (100k Ticks)** | Flat Heap (0 Leaks) | **100,000 ticks sustained / 0 leaks** | **Verified Stable Heap** |

### 2. Nanosecond Microbenchmarks (Release Profile)

Measured via native hardware timer suite (`crates/eidolon-server/tests/microbenchmarks.rs`):

| Operation | Implementation Path | Latency (ns/op) | Throughput |
| :--- | :--- | :--- | :--- |
| `Fixed64::saturating_mul` | 32.32 Fixed-Point Integer Multiply | **1.00 ns** | ~1.00 Billion ops/sec |
| `Fixed64::clamp` | Branchless Coordinate Clamping | **0.69 ns** | ~1.45 Billion ops/sec |
| `Fixed64::sqrt` | Integer Bitwise Square Root | **60.22 ns** | ~16.6 Million ops/sec |
| `Vec3Fix::distance_squared` | 3D Squared Euclidean Distance | **2.12 ns** | ~471 Million ops/sec |
| `Vec3Fix::batch_distance_squared_4x` | 4-Wide SIMD Lane Batching | **7.62 ns** (1.9 ns/lane) | ~131 Million batches/sec |
| `QuantizedCellCoord::quantize` | Branchless 44-bit Quantization | **1.06 ns** | ~945 Million ops/sec |
| `QuantizedCellCoord::pack` | 7-Byte Wire Bitpack (Coord+Yaw+Flags) | **0.75 ns** | ~1.33 Billion ops/sec |
| `QuantizedCellCoord::unpack` | 7-Byte Wire Bitstream Unpack | **0.84 ns** | ~1.18 Billion ops/sec |
| `BitWriter::write_bits` | Register-Width Bitstream Ingestion | **34.33 ns** | ~29.1 Million ops/sec |
| `BitReader::read_bits` | Register-Width Bitstream Extraction | **10.02 ns** | ~99.7 Million ops/sec |
| `SpatialHashGrid::query_radius_batched` | 4-Wide SIMD 9-Cell Neighborhood Query | **4,969 ns** (4.97 µs) | ~201 Thousand queries/sec |
| `kinematics::extrapolate` | Second-Order Intent Extrapolation | **9.00 ns** | ~111 Million ops/sec |
| `kinematics::should_dispatch_update` | Predictive Velocity Deadband Check | **4.53 ns** | ~220 Million ops/sec |
| `SpscPacketQueue::try_push + try_pop` | Bounded SPSC Queue Roundtrip | **62.43 ns** | ~16.0 Million ops/sec |

### 3. Simulation Latency Percentiles (2,000 CCU, 20 Hz Simulation)

| Percentile | Measured Duration (Release Profile) | Allocated 20 Hz Budget | Measured Headroom |
| :--- | :--- | :--- | :--- |
| **Minimum** | **2.67 ms (2,666 µs)** | 50.00 ms | **94.7% Headroom** |
| **Median (p50)** | **3.06 ms (3,061 µs)** | 50.00 ms | **93.9% Headroom** |
| **75th Percentile (p75)** | **3.23 ms (3,226 µs)** | 50.00 ms | **93.5% Headroom** |
| **95th Percentile (p95)** | **3.89 ms (3,888 µs)** | 50.00 ms | **92.2% Headroom** |
| **99th Percentile (p99)** | **4.75 ms (4,755 µs)** | 50.00 ms | **90.5% Headroom** |
| **Maximum** | **5.15 ms (5,147 µs)** | 50.00 ms | **89.7% Headroom** |

### 4. Extreme Density Cluster Scaling: Flash Mob Traffic Suppression

Simulating concentrated gatherings packed into a single 64-meter cell, contrasting raw 20 Hz broadcast demand against `eidolon` scheduled egress:

| Scenario | Density in AoI | Query | Reconcile | Unclamped Demand (20 Hz) | `eidolon` Scheduled Output | Traffic Suppression | Budget Status |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Sparse Encounter** | 5 entities | **0.33 µs** | **0.19 µs** | 1,900 B/s (1.9 KB/s) | **950.00 B/s (0.93 KB/s)** | **50.0%** | Conforms (<1.2 KB/s) |
| **50th % (Ambient)** | 10 entities | **0.49 µs** | **0.41 µs** | 3,000 B/s (2.9 KB/s) | **1,170.00 B/s (1.14 KB/s)** | **61.0%** | Conforms (<1.2 KB/s) |
| **75th % (Active Hub)**| 50 entities | **1.63 µs** | **1.85 µs** | 11,800 B/s (11.5 KB/s) | **1,170.00 B/s (1.14 KB/s)** | **90.1%** | Conforms (<1.2 KB/s) |
| **90th % (Chokepoint)**| 100 entities | **1.37 µs** | **2.23 µs** | 22,800 B/s (22.3 KB/s) | **1,170.00 B/s (1.14 KB/s)** | **94.9%** | Conforms (<1.2 KB/s) |
| **95th % (Major Raid)**| 250 entities | **3.09 µs** | **7.08 µs** | 55,800 B/s (54.5 KB/s) | **1,170.00 B/s (1.14 KB/s)** | **97.9%** | Conforms (<1.2 KB/s) |
| **99th % (Flash Mob)** | 500 entities | **6.11 µs** | **19.11 µs** | 110,800 B/s (108.2 KB/s) | **1,170.00 B/s (1.14 KB/s)** | **98.9%** | Conforms (<1.2 KB/s) |

*Hardware Environment: Apple M4 (10 cores, NEON SIMD), macOS Darwin 24.3.0 arm64, `rustc 1.98.1` (`--release`). All hot tick simulation paths execute with zero runtime heap allocations. Complete disclosure in [`docs/BENCHMARKS.md`](./docs/BENCHMARKS.md).*

---

## Workspace Crates

`eidolon` is organized as a modular Cargo workspace built entirely from first principles:

```text
eidolon/
├── crates/
│   ├── eidolon-core/        # Fixed-point math, 44-bit quantization, dead reckoning & identity
│   ├── eidolon-net/         # UDP transport, register bitpacking, packet channels & crypto
│   ├── eidolon-spatial/     # Spatial hash grid, intrusive slot maps & dynamic 3-tier AoI
│   ├── eidolon-world/       # Seamless zoned world, durable journal, dungeons & hibernation
│   └── eidolon-server/      # Headless server binary, tick loop coordinator & Agones hooks
├── docs/                    # Public technical and architectural specifications
├── Cargo.toml               # Workspace manifest
└── LICENSE                  # Business Source License 1.1 ($1M Indie Exemption)
```

| Crate | Core Architecture & Responsibilities |
| :--- | :--- |
| **`eidolon-core`** | Float-free deterministic arithmetic, 32.32 fixed-point vectors (`Fixed64`, `Vec3Fix`), 44-bit coordinate quantization, 1-byte yaw, intent dead reckoning extrapolation FSM, and 3-tier `AccountId -> SessionTicket -> CharacterId` capability tokens. |
| **`eidolon-net`** | Register-width bitstream reader/writer, 12-byte zero-copy packet framing, sequenced unreliable channels, ordered reliable channels with sliding-window selective ACKs, first-principles SHA-256/HMAC-SHA256, pluggable `CryptoProvider`, and token-bucket rate policers. |
| **`eidolon-spatial`** | Cache-conscious 2D/3D spatial hash grid with 64-byte aligned bucket headers, intrusive slot-map indexing, 4-wide SIMD batched radius queries, dynamic 3-tier AoI frequency state machines, and pre-allocated double-buffered interest sets. |
| **`eidolon-world`** | World manager coordinating seamless zone boundaries with 16m overlapping seams, atomic in-memory entity handoffs, append-only `DurableFileJournal` with POSIX `fdatasync`, ephemeral dungeon instances (<50ms allocation), and cold account hibernation (<256B). |
| **`eidolon-server`** | Headless server binary. Integrates native non-blocking UDP I/O worker threads with a synchronous 20 Hz tick coordinator, target-instant drift-free pacing, zero-allocation bounded SPSC queues, Prometheus telemetry, and Agones Kubernetes lifecycle hooks. |

---

## Quickstart & Developer Guide

### Prerequisites
- [Rust Toolchain](https://www.rust-lang.org/) (1.80.0 or higher recommended, 2021 edition).
- Zero external C/C++ build dependencies, CMake, or system libraries required.

### 1. Build the Workspace
Compile the entire workspace in release mode with maximum optimization:

```bash
cargo build --release
```

### 2. Run the Full Test Suite
Execute unit, integration, and chaos test suites across all 5 workspace crates:

```bash
cargo test --all-targets --all-features
```

### 3. Run the Microbenchmark Battery
Benchmark fixed-point math, coordinate quantization, bitpacking, and SIMD spatial queries on your local hardware:

```bash
cargo test -p eidolon-server --test microbenchmarks -- --nocapture
```

### 4. Run the 2,000 CCU Load Simulation Harness
Execute the authoritative multi-client load test verifying 20 Hz cadence and sub-1.2 KB/s wire egress:

```bash
cargo test -p eidolon-server --test simulation_harness -- --nocapture
```

### 5. Verify Architectural Invariants & Compliance
Run the automated compliance test gate verifying zero third-party dependencies, memory safety lints, and stealth boundaries:

```bash
cargo test -p eidolon-server --test compliance
```

---

## Technical Documentation

In-depth engineering specifications, mathematical proofs, and operational runbooks are published in `docs/`:

* **[System Architecture (`docs/ARCHITECTURE.md`)](./docs/ARCHITECTURE.md):** System topology, tick loop scheduling, time budget enforcement, and Agones lifecycle.
* **[Wire Protocol & Byte Math (`docs/WIRE_PROTOCOL.md`)](./docs/WIRE_PROTOCOL.md):** 32.32 fixed-point equations, 44-bit coordinate quantization, 1-byte yaw, and 12-byte packet framing.
* **[Spatial Partitioning & AoI (`docs/SPATIAL_PARTITIONING.md`)](./docs/SPATIAL_PARTITIONING.md):** Spatial hash grid, Struct-of-Arrays memory layout, intrusive slot-map, and 3-tier AoI state machines.
* **[Zones & Instancing (`docs/ZONES_AND_INSTANCING.md`)](./docs/ZONES_AND_INSTANCING.md):** Seamless 16-meter boundary seams, ephemeral dungeon pools, scale-to-zero gacha raids, and cold account hibernation.
* **[Disaster Recovery & RPO/RTO Targets (`docs/DISASTER_RECOVERY.md`)](./docs/DISASTER_RECOVERY.md):** Physical `fdatasync` commit contract, crash recovery runbooks, and quantitative RPO/RTO metrics.
* **[Benchmarks & Verification (`docs/BENCHMARKS.md`)](./docs/BENCHMARKS.md):** Empirical nanosecond microbenchmark results, 2,000 CCU load simulation, and memory leak audit proofs.
* **[Cloud Infrastructure Cost Analysis (`docs/COST_ANALYSIS.md`)](./docs/COST_ANALYSIS.md):** Public cloud egress financial models, headcount savings matrix (5 to 1,000,000 CCU), and EoS perpetual maintenance economics.
* **[Operations, Observability & Security (`docs/OPERATIONS_AND_SECURITY.md`)](./docs/OPERATIONS_AND_SECURITY.md):** Adversarial threat modeling, SRE phase latency telemetry, protocol semantic versioning, and Agones lifecycle hooks.
* **[Linux Kernel & Low-Level UDP Tuning (`docs/LINUX_KERNEL_TUNING.md`)](./docs/LINUX_KERNEL_TUNING.md):** Production OS socket buffer tuning, NIC multi-queue RSS, and Agones host networking runbook.
* **[Per-Player Strict Memory Budget (`docs/MEMORY_BUDGET.md`)](./docs/MEMORY_BUDGET.md):** Byte-exact accounting per player connection (<48 KB/CCU) and bounded memory proofs for 100,000 CCU.

---

## Companion Ecosystem: `pak-delta`

MMO network consumption spans two distinct operational domains:

```text
                            Total MMO Bandwidth Challenge
                                           │
         ┌─────────────────────────────────┴─────────────────────────────────┐
         ▼                                                                   ▼
┌─────────────────────────────────┐                         ┌─────────────────────────────────┐
│       Live Gameplay Wire        │                         │     Client Patches & Assets     │
│   (Positions, combat, ticks)    │                         │   (3D models, textures, audio)  │
│                                 │                         │                                 │
│       Target: eidolon           │                         │       Target: pak-delta         │
│    (Sub-1KB/s UDP streaming)    │                         │  (Archive-aware delta patching) │
└─────────────────────────────────┘                         └─────────────────────────────────┘
```

While `eidolon` optimizes runtime simulation and wire streaming, [`pak-delta`](https://github.com/mikev-lab/pak-delta) handles client game distribution by producing byte-level delta patches across uncompressed and compressed asset containers (Unreal `.pak`, Unity bundles, ZIP), ensuring patch updates download only modified bytes.

`eidolon-world` natively integrates `pak-delta` patch negotiation via `PatchNegotiator` and `ZoneAssetRequirement`, asserting that clients possess required asset versions prior to zone or dungeon transitions.

---

## Engineering Governance & Safety

`eidolon` enforces strict engineering standards across the repository:
- **Zero Third-Party Runtime Dependencies:** Everything is native Rust standard library (`std` / `core`), eliminating supply-chain attack vectors and dependency rot.
- **Absolute Memory Safety:** The entire workspace enforces `#![deny(unsafe_code)]` at the compiler level.
- **Panic-Free Production Invariant:** Production crates never call `unwrap()` or `expect()`, using structured `Result<T, E>` error handling.
- **Continuous Sanitizer Auditing:** Continuous integration runs automated AddressSanitizer (ASan) and UndefinedBehaviorSanitizer (UBSan) suites.

---

## Licensing & Fair-Source Model

`eidolon` is published under the **Business Source License 1.1 ([BSL 1.1](./LICENSE))** with an explicit **Indie Exemption**:

* **Free for Indies & Individuals:** 100% free of charge for individuals, educational institutions, non-profits, and commercial entities generating **under $1,000,000 USD** in gross annual revenue.
* **Commercial Enterprise Tier:** Organizations generating $\ge \$1,000,000\text{ USD}$ gross annual revenue require a commercial enterprise agreement.
* **Automatic Open-Source Conversion:** On the Change Date (**2030-01-01**), the codebase automatically converts to the permissive **Apache License, Version 2.0**.

For full legal terms, review the [LICENSE](./LICENSE) file.

---

<div align="center">
  <sub>eidolon MMO World Server Engine • Engineered by <a href="https://github.com/mikev-lab">Michael Valdez (mikev-lab)</a>.</sub>
</div>
