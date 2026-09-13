<div align="center">

# eidolon

**High-concurrency, low-bandwidth (<1 KB/s design target) zoned & instanced MMO server engine in Rust.**

[![Status: Foundational Architecture](https://img.shields.io/badge/Status-Foundational_Architecture-informational.svg)](#development-status)
[![License: BSL 1.1](https://img.shields.io/badge/License-BSL_1.1_(Fair_Source)-blue.svg)](./LICENSE)
[![Indie Grant: <$1M Free](https://img.shields.io/badge/Indie_Grant-%3C$1M_Free-success.svg)](./LICENSE)
[![Language: Rust](https://img.shields.io/badge/Language-Rust-orange.svg)](https://www.rust-lang.org/)
[![Target Wire Budget](https://img.shields.io/badge/Design_Target-%3C1_KB%2Fs_Wire_Budget-blueviolet.svg)](#the-engineering-problem-the-mmo-egress-trap)

<p align="center">
  <a href="#overview">Overview</a> •
  <a href="#the-engineering-problem-the-mmo-egress-trap">The Problem</a> •
  <a href="#architectural-design-sub-1kbs-wire-budget">Design Targets</a> •
  <a href="#world-topology">World Topology</a> •
  <a href="#gacha--long-tail-eos-preservation">Gacha & EoS Preservation</a> •
  <a href="#planned-workspace-architecture">Crates</a> •
  <a href="#licensing--fair-source">License</a>
</p>

</div>

---

> [!NOTE]
> **Project Status: Initial Architectural Foundation.**  
> `eidolon` is in active pre-v0.1 foundational development. The specifications, wire calculations, and topology diagrams below outline the architectural blueprint and mathematical models currently being implemented.

---

## Overview

**eidolon** is an open-architecture, headless game server engine in active development, engineered from first principles in Rust for large-scale persistent worlds and scale-to-zero live-service games.

Most multiplayer architectures present difficult compromises:
1. **Matchmaking session frameworks** that excel at short rounds (e.g. 5v5 lobbies) but struggle with persistent, contiguous open worlds.
2. **Monolithic legacy MMO backends** that require large monthly cloud infrastructure budgets, complex third-party middleware, and suffer from garbage collection latency spikes.

`eidolon` is being engineered from first principles around an architectural thesis: **design a server engine that costs virtually $0/month for a small group of players (fitting within Google Cloud's free `e2-micro` tier), while possessing the deterministic efficiency to scale horizontally to millions of concurrent users on Kubernetes without an architectural rewrite.**

---

## The Engineering Problem: The MMO Egress Trap

In large-scale multiplayer games, **cloud network egress (not CPU compute) is often the single largest operational expense.**

Standard cloud providers (AWS, GCP, Azure) bill public internet egress between **$0.05 and $0.12 per gigabyte**. The table below illustrates the economic motivation for aggressive byte quantization:

| Metric | Unoptimized Baseline (20 KB/s) | **eidolon Design Target (0.8 KB/s)** |
| :--- | :--- | :--- |
| **Bandwidth per Player** | 20 KB/s (160 kbps) | **0.8 KB/s (6.4 kbps)** |
| **Throughput at 100,000 CCU** | 2.0 GB/s (16 Gbps) | **80 MB/s (640 Mbps)** |
| **Throughput at 1,000,000 CCU** | 20.0 GB/s (160 Gbps) | **800 MB/s (6.4 Gbps)** |
| **Monthly Egress (1M CCU @ 16h/day)** | ~34.5 Petabytes | **~1.38 Petabytes** |
| **Estimated Cloud Egress (at $0.07/GB)** | **~$2,400,000 / month** | **~$96,000 / month** |

> **The Architectural Takeaway:** Halving packet size cuts cloud egress expenses in half. Compressing updates down to under 1 KB/s makes independent, community-hosted, and long-tail live-service games economically viable without requiring venture subsidies.

---

## Architectural Design: Sub-1KB/s Wire Budget

Classic online role-playing games like *World of Warcraft* (2004) famously functioned over 56k dial-up modems (~4–5 KB/s). `eidolon` applies these lessons alongside modern Data-Oriented Design to target an average wire footprint of **0.5 to 1.2 KB/s per client**:

### 1. Intent-Based Dead Reckoning
Streaming raw IEEE 754 32-bit coordinates $[X, Y, Z, \text{Yaw}]$ every tick quickly saturates network queues ($36\text{ bytes} \times 30\text{ entities} \times 20\text{ Hz} = 21.6\text{ KB/s}$). 
`eidolon` transmits **intent and velocity vectors** only when an entity accelerates, turns, or changes state. Server and client run deterministic extrapolation between updates, eliminating up to **85% of movement packets**.

### 2. Spatial Frequency Tiers (Dynamic AoI)
Entities do not update at uniform frequencies:
* **Immediate Tier (< 10m):** High-frequency updates at **10 Hz** (combat targets, close interactions).
* **Mid Tier (10m - 50m):** Interpolated updates at **2 Hz** (patrolling NPCs, ambient players).
* **Horizon Tier (> 50m):** Event-only scheduling (entering/exiting visibility, major state events).

### 3. Bit-Packed Coordinate Quantization
Instead of sending uncompressed floating-point values, coordinates are quantized relative to local 64-meter spatial grid cells:
* **Horizontal Axis ($X, Z$):** Quantized to 16-bit integers ($65,536$ discrete divisions $\approx 0.97\text{ mm}$ resolution).
* **Vertical Axis ($Y$):** Quantized to 12-bit integers ($4,096$ divisions across a 32m vertical band $\approx 7.8\text{ mm}$ resolution).
* **Yaw / Heading:** Quantized to a single byte ($256$ discrete angles $\approx 1.4^\circ$ resolution).
* **Target Transform Footprint:** Position, orientation, and status flags fit within **6 to 7 bytes**.

### 4. Zero-Copy Bitstream Serialization
State bitflags are packed into single-byte bitmasks, variable-length integers (varints) are used for entity identifiers, and packet buffers are read directly from 64-bit integer registers (`u64`) without dynamic heap allocations.

---

## World Topology

`eidolon` is designed around a **hybrid zoned & instanced architecture**:

```
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

* **Seamless Persistent Zones:** Persistent world zones are divided into spatial hash grids. When an entity crosses a boundary, ownership is transferred in memory between spatial grids without disconnecting the client or triggering a loading screen.
* **Ephemeral Dungeon Instances:** Dungeons, arenas, and housing interiors are lightweight private room instances allocated on demand when a party enters a portal and deallocated immediately when vacated.

---

## Gacha & Long-Tail EoS Preservation

Beyond MMOs, `eidolon` addresses the **"End-of-Service" (EoS) cliff** common in mobile gacha games, card battlers, and co-op RPGs.

Live-service games frequently shut down because **fixed server hosting overhead exceeds the revenue of the remaining player base**. Paying for always-on server clusters and patch CDNs for 500 loyal daily players can burn thousands of dollars monthly.

`eidolon` enables games to transition into an ultra-low-cost **Perpetual Maintenance Mode**:

1. **Scale-to-Zero Co-op Raids:** Battle rooms spin up in `<50ms` only when players initiate matchmaking. When zero matches are running, active server compute drops to absolute zero ($0 cost).
2. **Cold-State Account Hibernation:** Inactive player rosters, pity counters, and inventories serialize into cold storage snapshots (costing `<$0.0001` per dormant account/month).
3. **Low-Bandwidth Resilience:** Sub-1KB/s wire traffic coupled with [`pak-delta`](https://github.com/mikev-lab/pak-delta) micro-patching minimizes the operational bandwidth footprint.

---

## Planned Workspace Architecture

`eidolon` is organized as a modular Rust Cargo workspace:

```text
eidolon/
├── crates/
│   ├── eidolon-core/        # Math primitives, quantization tables & dead reckoning
│   ├── eidolon-net/         # UDP transport, register bitpacking & packet protocol
│   ├── eidolon-spatial/     # Spatial hash grid & dynamic AoI frequency tiers
│   ├── eidolon-world/       # Seamless zoned world & ephemeral dungeon room manager
│   └── eidolon-server/      # Headless server binary, tick loop runner & Agones hooks
├── Cargo.toml               # Workspace configuration
└── LICENSE                  # Business Source License 1.1 ($1M Indie Exemption)
```

| Crate | Planned Responsibilities |
| :--- | :--- |
| **`eidolon-core`** | Fixed-point vector algebra (`Vec3Fix`), coordinate quantizers, and dead reckoning extrapolation algorithms. Designed to be imported by game clients for deterministic prediction. |
| **`eidolon-net`** | Low-latency UDP transport layer. Register-width bitstreams, packet framing, sequenced unreliable channels, and ordered reliable channels with backpressure. |
| **`eidolon-spatial`** | Cache-conscious 2D/3D spatial hash grid with 64-byte aligned bucket headers and dynamic AoI frequency tier state machines. |
| **`eidolon-world`** | The world manager. Coordinates seamless zone boundaries, atomic in-memory handoffs, and lifecycle management for ephemeral instanced rooms. |
| **`eidolon-server`** | The production headless server binary. Integrates Tokio async I/O worker threads with a synchronous 20 Hz simulation tick loop and Agones Kubernetes lifecycle hooks. |

---

## Companion Ecosystem: `pak-delta`

MMO network consumption spans two distinct operational domains:

```
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

While `eidolon` focuses on the runtime simulation and wire protocol, [`pak-delta`](https://github.com/mikev-lab/pak-delta) handles game distribution by producing byte-level delta patches across uncompressed and compressed asset containers (Unreal `.pak`, Unity bundles, ZIP), ensuring patch downloads update only modified bytes.

---

## Licensing & Fair-Source Model

`eidolon` is published under the **Business Source License 1.1 ([BSL 1.1](./LICENSE))** with an explicit **Indie Exemption**.

* **Free for Indies & Individuals:** 100% free of charge for individuals, educational institutions, non-profits, and commercial entities generating **under $1,000,000 USD** in gross annual revenue.
* **Commercial Enterprise Tier:** Organizations generating $\ge \$1,000,000\text{ USD}$ gross annual revenue require a commercial enterprise agreement.
* **Automatic Open-Source Conversion:** On the Change Date (**2030-01-01**), the codebase automatically converts to **Apache License, Version 2.0**.

For the full legal parameters, please review the [LICENSE](./LICENSE) file.

---

## Development Status

`eidolon` is currently in active foundational development.

### Phase 1 Progress
- [x] Architecture & Wire Protocol Specification
- [x] BSL 1.1 Fair-Source Licensing with $1M Indie Exemption
- [x] Engineering Governance & Production Invariants (`AGENTS.md`)
- [x] Workspace Root Cargo Manifest & Crate Stubs (`crates/`)
- [x] Automated Governance & Compliance Test Harness (`compliance.rs`)
- [x] GitHub Actions CI Workflow (`.github/workflows/ci.yml`)

### Phase 2 Progress (Core Wire Math & Kinematics)
- [x] Deterministic 32.32 Fixed-Point Math (`Fixed64`, `Vec3Fix`)
- [x] Asymmetric Coordinate Quantization (16-bit X/Z, 12-bit Y)
- [x] 1-Byte Discrete Heading & Shortest-Arc Rollover (`QuantizedYaw`)
- [x] Compact 7-Byte Wire Bitpacking (Coordinates + Yaw + Flags)
- [x] Second-Order Intent Dead Reckoning & Extrapolation FSM
- [x] Tier 1 Deterministic Parity & 10,000-Tick Verification Suite

---

<div align="center">
  <sub>Engineered by <a href="https://github.com/mikev-lab">Michael Valdez (mikev-lab)</a>.</sub>
</div>
