# eidolon Production Infrastructure Roadmap

**Engineering Roadmap: Transitioning from Pre-Production Engine to Production-Ready MMO Infrastructure**

---

## 1. Executive Summary & The Architectural Pivot

The development of `eidolon` has crossed a decisive milestone. The core mathematical, simulation, spatial, and wire transport subsystems (Phases 1 through 7) are implemented, benchmarked, and verified:
- **Core Networking & Simulation Engine:** Verified 20 Hz deterministic simulation loop, sub-millimeter quantization (0.977 mm error), second-order dead reckoning, sub-8 microsecond spatial queries under 500-entity flash mobs, and strict wire egress clamping (<1.14 KB/s L7 / 1.18 KB/s L3/L4 wire payload).
- **The Next Engineering Challenge:** The barrier to deploying a commercial, large-scale MMO is not micro-optimizing CPU cycles from 7 ns to 5 ns. The challenge is **distributed systems, persistence, security, and operations under hostile real-world conditions**.

The roadmap is structured into two eras:
1. **Core Engine Architecture (Phases 1-7):** Completed and verified (20% of total MMO architecture).
2. **Production Infrastructure Engineering (Phases 8-13):** Active implementation roadmap (80% of total MMO architecture).

```
┌───────────────────────────────────────────────────────────────────────────────┐
│                    MASTER PRODUCTION INFRASTRUCTURE ROADMAP                    │
├─────────────┬───────────────────────────────────────────────────┬─────────────┤
│ Phase       │ Subsystem Focus                                   │ Priority    │
├─────────────┼───────────────────────────────────────────────────┼─────────────┤
│ Phase 1-7   │ Core Simulation, Quantization, Transport & AoI    │ COMPLETED   │
│ Phase 8     │ Distributed Authority, Session Security & Quotas  │ COMPLETED   │
│ Phase 9     │ Authoritative Persistence & Crash Reconstruction  │ COMPLETED   │
│ Phase 10    │ End-to-End Backpressure & Capacity Admission      │ P1 Launch   │
│ Phase 11    │ Production Observability & Distributed Tracing    │ P1 Launch   │
│ Phase 12    │ Real Network Impairment, Fuzzing & Multi-Server   │ P1 Launch   │
│ Phase 13    │ Platform Maturity, Kernel Tuning & Formal Verif.  │ P2 Maturity │
└─────────────┴───────────────────────────────────────────────────┴─────────────┘
```

---

## 2. Priority Classification Framework

Following strict distributed systems engineering discipline, deliverables are categorized by operational criticality:

- **P0 (Critical Prerequisites):** Hard prerequisites before exposing the server to real players or public networks. Failure causes lost state, dupe exploits, unauthorized takeovers, split-brain divergence, or unrecoverable crashes.
- **P1 (Commercial Launch Gate):** Hard requirements for an open commercial production launch. Includes distributed multi-server load testing, real-world network degradation, end-to-end backpressure, and zero-downtime rolling upgrades.
- **P2 (Platform Maturity & Excellence):** Engineering maturity standards including multi-architecture CPU profiling, low-level OS kernel socket tuning, and formal property-based verification.

---

## 3. Master Phase Specifications

```
                                  PIPELINE DATAFLOW
                                  
   Client UDP Packet
          │
          ▼
   ┌──────────────┐     Token Fencing / Epochs
   │  Phase 8     │ ───────────────────────────────┐
   │  Auth & Gate │                                │
   └──────┬───────┘                                │
          │ Bounded Ingress                        ▼
          ▼                                 ┌──────────────┐
   ┌──────────────┐   Transactional WAL     │  Phase 9     │
   │  Phase 10    │ ──────────────────────> │  Persistence │
   │  Tick & Flow │                         │  & Recovery  │
   └──────┬───────┘                         └──────────────┘
          │ Microsecond Telemetry
          ▼
   ┌──────────────┐
   │  Phase 11    │   Correlated Span Context
   │  Telemetry   │ ──────────────────────────────────────────> Prometheus / OTel
   └──────┬───────┘
          │
          ▼
   ┌──────────────┐   Compound Impairment (Loss, Jitter, RTT)
   │  Phase 12    │ <─────────────────────────────────────────> Chaos Engine
   │  Chaos & Soak│
   └──────────────┘
```

---

### Phase 8: Distributed Authority, Session Security & Malicious-Client Resistance (P0 - Completed & Verified)

*Objective: Establish uncompromised cryptographic session binding, deterministic authority epochs, partition fencing, and per-connection resource limits to guarantee that malicious clients cannot destabilize legitimate gameplay.*

- [x] **Milestone 8.1: Cryptographic Session Handshake & Replay Protection**
  - [x] Ephemeral session keys negotiated via native HMAC-SHA256 challenge-response during UDP association.
  - [x] Circular 64-bit monotonic sequence replay window: reject duplicate or stale packets outside the valid replay horizon.
  - [x] Session nonce rotation and time-to-live (TTL) expiration guards.
- [x] **Milestone 8.2: Malicious-Client Resource Fencing & Anti-DoS Quotas**
  - [x] Hard per-connection ingress rate limits: maximum 40 packets/sec and 32 KB/sec per client socket before immediate drop.
  - [x] Entity allocation quotas: strict limits on entity spawning, interaction rate, and inventory mutations.
  - [x] Memory isolation: hard ceiling of 64 KB memory allocation per active connection.
  - [x] CPU budget enforcement: untrusted bitstream parsing bounded to <5 microseconds per packet. Guaranteed invariant: no single malicious client can consume more than 0.01% of a tick budget.
- [x] **Milestone 8.3: Deterministic Authority Epochs & Reconnect Fencing**
  - [x] Four-part authority tuple: `[account_id, session_id, authority_epoch, sequence_number]`.
  - [x] Stale session rejection: server nodes unconditionally reject packets with `epoch < current_epoch`.
  - [x] Reconnect fencing: when a client reconnects to Server B after Server A failure, increment `authority_epoch`. Any delayed packets arriving at Server A or B from older epochs are dropped immediately.
- [x] **Milestone 8.4: Multi-Node Zone Partition Semantics & Split-Brain Prevention**
  - [x] Explicit distributed partition handling for severed inter-zone links (`Zone A <-> Zone B`).
  - [x] Lease-based spatial boundary ownership: zone servers maintain renewable short-term leases on boundary entities.
  - [x] Network split resolution: if partition lasts >3 heartbeats, boundary entities transition to read-only frozen state; migration transactions automatically roll back to the authoritative home zone.
  - [x] Zero duplicate entities: guaranteed single-writer invariant across cluster boundaries.
- [x] **Milestone 8.5: Wire Protocol Versioning & Rolling Compatibility**
  - [x] Implement runtime protocol negotiation in packet headers (`MAGIC = 0x4549`, `PROTOCOL_VERSION = 1`).
  - [x] Automated compatibility test matrix:
    * Client v1 <-> Server v1 (Standard baseline).
    * Client v1 <-> Server v2 (Backward-compatible deprecation).
    * Client v2 <-> Server v1 (Graceful feature degradation or typed upgrade requirement).
  - [x] Mixed-cluster wire translation during rolling deployments without disconnecting active players.

---

### Phase 9: Authoritative Persistence, Transactional Consistency & Crash Recovery (P0 - Completed & Verified)

*Objective: Implement a rock-solid, crash-resilient persistence architecture with an explicit consistency pipeline, eliminating item duplication exploits, rollbacks, and progress loss during node crashes or database outages.*

- [x] **Milestone 9.1: Authoritative Consistency Pipeline & Write-Ahead Journal**
  - [x] Explicit four-stage state flow:
    $$\text{Simulation Tick} \longrightarrow \text{Authoritative State} \longrightarrow \text{Durable Write-Ahead Journal (WAL)} \longrightarrow \text{Periodic Checkpoint / DB}$$
  - [x] Asynchronous, non-blocking append-only binary journal operating independently from the 20 Hz tick thread.
  - [x] Flush acknowledgment semantics: mutations affecting durable state (trades, drops, quest completions) return client ACK only after journal synchronization.
- [x] **Milestone 9.2: Double-Spend Prevention & Multi-Session Fencing**
  - [x] Atomic transactional boundaries for currency, item exchanges, and inventory transfers.
  - [x] Distributed lock tokens with generation counters (`GenerationLock`) preventing dual-login duplicate exploits.
  - [x] Rejection of overlapping session writes: old session cannot overwrite newer state during rapid reconnects.
- [x] **Milestone 9.3: Asynchronous Database Outage Resilience**
  - [x] Graceful degradation under prolonged (30-second) database or storage unavailability.
  - [x] Bounded in-memory journal buffer: queues persistent mutations without blocking the 20 Hz simulation loop.
  - [x] Exponential backoff reconnect and automated replay of pending journal entries upon database restoration.
- [x] **Milestone 9.4: Disposable Zone Server Crash Reconstruction**
  - [x] Crash recovery architecture: zone worker nodes are fully disposable and stateless in compute.
  - [x] Automated state reconstruction: if `zone-worker-17` crashes, neighboring or replacement nodes reconstruct zone state from the durable journal and active client session snapshots.
  - [x] Automated chaos test suite: simulating process kills (`SIGKILL`), sudden machine drops, and partial network partitions with 100% state recovery and zero entity duplication.
- [x] **Milestone 9.5: Cold Account Hibernation to Hot Hydration Lifecycle**
  - [x] Production binary serialization for dormant accounts (<256 bytes per snapshot).
  - [x] Sub-5ms hydration pipeline restoring characters, inventories, and cooldowns directly into active zone spatial grids upon client login.

---

### Phase 10: End-to-End Backpressure, Capacity Admission & Clock Discipline (P1)

*Objective: Unify network queues, simulation ticks, and egress scheduling into a bounded backpressure pipeline, while establishing strict clock discipline against NTP drift and VM migration pauses.*

- **Milestone 10.1: Unified End-to-End Backpressure Pipeline**
  - Continuous backpressure dataflow:
    $$\text{UDP Socket} \longrightarrow \text{Ingress Queue} \longrightarrow \text{Simulation} \longrightarrow \text{AoI Manager} \longrightarrow \text{Replication Scheduler} \longrightarrow \text{Egress Queue} \longrightarrow \text{UDP Socket}$$
  - Zero unbounded queues anywhere in the engine.
  - Defined stage behavior under saturation:
    * Ingress Queue: Drop stale unsequenced movement packets; preserve ordered reliable packets until timeout.
    * Replication Scheduler: Shed low-frequency AoI tiers (Horizon -> Mid -> Immediate).
    * Egress Queue: Drop oldest unacked unreliable transforms; apply socket backpressure.
- **Milestone 10.2: Hierarchical Capacity Admission Control & Graceful Saturation Degradation**
  - Formal priority shedding hierarchy under extreme entity density:
    $$\text{Critical Combat Events} > \text{Nearby Movement (<10m)} > \text{Mid-Range Movement (<50m)} > \text{Far State} > \text{Cosmetics}$$
  - Controlled degradation policy: bandwidth pressure automatically reduces update frequency, scales quantization precision, and pages distant entities while maintaining combat responsiveness.
- **Milestone 10.3: Simulation Clock Discipline & Time Warp Protection**
  - Strict separation of monotonic simulation time (`Instant`) from wall-clock UTC time (`SystemTime`).
  - NTP step immunity: simulated ticks proceed strictly via hardware monotonic cycles; wall-clock jumps (+5s or -2s) cannot cause tick skips or simulation time acceleration.
  - Host virtualization pause recovery: detect hypervisor suspend/resume events and perform graceful multi-tick catch-up clamping rather than spiral-of-death tick cascades.

---

### Phase 11: Production Observability, High-Cardinality Telemetry & Tracing (P1)

*Objective: Equip the engine with production-grade telemetry, microsecond-accurate phase timing, bounded-cardinality Prometheus metrics, and correlated distributed tracing.*

- **Milestone 11.1: Production Service Level Objectives (SLOs) & Prometheus Metrics**
  - Server-level metrics: tick duration percentiles (p50, p95, p99, p99.9), ingress/egress packets per second, bytes per client, queue depths, retransmission rates, and allocator memory.
  - Zone-level metrics: active entity count, observer count, hotspot density index, and border migration rate.
  - Bounded label cardinality: strictly avoid per-player metric labels to protect Prometheus from memory exhaustion; aggregate per-player metrics into bounded histogram buckets.
- **Milestone 11.2: Correlated Distributed Tracing**
  - End-to-end trace context propagation across the request lifecycle:
    $$\text{Client Input} \longrightarrow \text{Gateway} \longrightarrow \text{Zone Server} \longrightarrow \text{Simulation Tick} \longrightarrow \text{AoI Query} \longrightarrow \text{Replication Egress}$$
  - Micro-trace correlation tuple: `[session_id, tick_id, entity_id, zone_id, authority_epoch]`.
  - Rapid root-cause debugging for player rubber-banding, input drops, or migration stalls.
- **Milestone 11.3: SRE Operational Runbooks & Automated Alerting Rules**
  - Detailed operational runbooks for cluster alerts: tick overruns, network partition alarms, journal write delays, and backpressure drops.
  - Agones integration health checks: automated pod draining and replacement when health thresholds degrade.

---

### Phase 12: Real Network Impairment, Exhaustive Fuzzing & Multi-Server Chaos (P1)

*Objective: Subject the engine to adversarial network environments, multi-server distributed topologies, deep fuzzing corpuses, and week-long soak testing.*

- **Milestone 12.1: Real-World Compound Network Impairment Suite**
  - Synthetic network impairment harness testing combined failure modes:
    * Packet loss: 0.1%, 1.0%, 2.0%, and 5.0% continuous and burst packet loss.
    * Latency & Jitter: 20ms to 200ms round-trip times with 150ms synthetic jitter.
    * Packet reordering and packet duplication.
    * Compound torture test: 5% burst loss + 150ms RTT + server tick load spike.
- **Milestone 12.2: Deep Packet Parser Fuzzing Corpus**
  - Millions of hostile, malformed payloads generated against:
    * `BitReader` and register shift boundaries.
    * LEB128 varint decoders (testing 10-byte anti-DoS bounds).
    * Circular 16-bit sequence and ACK bitfield unpackers.
    * Coordinate quantizers and entity ID discriminants.
  - Strict invariant: zero panics, zero memory leaks, zero infinite loops, and typed error rejections across 100% of fuzz inputs.
- **Milestone 12.3: Distributed Multi-Server Load Harness (10k to 100k CCU)**
  - Distributed multi-process test harness running across multiple nodes:
    * Zone Servers (Zone A, Zone B, Zone C).
    * Instance Servers (dungeons and co-op rooms).
    * Gateway and Session Coordinators.
  - Scaled concurrency tests: 10,000 to 100,000 synthetic clients executing movement, cross-zone travel, combat actions, and mass reconnect bursts.
- **Milestone 12.4: Multi-Day Continuous Soak Testing (24h / 72h / 7-Day)**
  - Sustained execution under load for 24 hours, 72 hours, and 7 consecutive days.
  - Rigorous assertions on:
    * Resident Set Size (RSS) memory stability (zero memory leaks).
    * Fixed-capacity memory pool stability (zero retained-state growth).
    * Long-term tick cadence stability (zero clock drift).
- **Milestone 12.5: Zero-Downtime Rolling Upgrade Verification**
  - Mixed-version cluster execution: Version N and Version N+1 running concurrently.
  - Seamless player migration between mixed-version zone servers without client disconnects.
  - Verified persistence schema migration and wire protocol negotiation during rolling rollouts.

---

### Phase 13: Platform Maturity, Kernel Tuning & Formal Verification (P2)

*Objective: Maximize hardware performance across CPU architectures, provide production OS kernel tuning guides, validate wire egress via eBPF/pcap, and formally verify transport invariants.*

- **Milestone 13.1: Multi-Platform Architecture & Benchmark Matrix**
  - Comprehensive benchmarking across diverse CPU microarchitectures:
    * AMD EPYC x86-64 (AVX2 / AVX-512).
    * Intel Xeon x86-64.
    * ARM64 Server (AWS Graviton, Ampere Altra).
    * Apple Silicon (M-series baseline).
  - Characterize cache hierarchy performance, branch prediction efficiency, and SIMD execution across platforms.
- **Milestone 13.2: Linux Kernel & Low-Level UDP Tuning Guide**
  - Production OS configuration runbook:
    * Socket buffer sizing (`SO_RCVBUF`, `SO_SNDBUF`).
    * Syscall batching considerations (`recvmmsg`, `sendmmsg`).
    * NIC multi-queue configuration, Receive Side Scaling (RSS), and CPU affinity.
    * MTU sizing, fragmentation prevention, and cloud ENA driver tuning.
- **Milestone 13.3: L3/L4/L7 Packet Capture & Interface Accounting**
  - Empirical wire validation using `tcpdump` and eBPF kernel tracing.
  - Full accounting reconciliation:
    $$\text{Application Payload (L7)} \longleftrightarrow \text{UDP Header (8B)} \longleftrightarrow \text{IPv4 Header (20B)} \longleftrightarrow \text{Physical Ethernet Frame (L2)}$$
  - Definitive third-party auditable verification of the <1.2 KB/s wire thesis.
- **Milestone 13.4: Per-Player Strict Memory Budget Specification**
  - Explicit byte-level accounting per connected player:
    * Active Connection Context: $C$ bytes.
    * Kinematic & Entity State: $E$ bytes.
    * AoI Visibility Relationships: $A$ bytes.
    * Reliable Channel Retransmission Buffers: $R$ bytes.
  - Hard upper bound proof: guaranteed ceiling of memory required for 100,000 players.
- **Milestone 13.5: Security Sanitizers & Miri Audits**
  - Continuous integration execution under AddressSanitizer (ASan) and UndefinedBehaviorSanitizer (UBSan).
  - Exhaustive Miri verification (`cargo miri test`) asserting complete memory safety and absence of undefined behavior.
- **Milestone 13.6: Property-Based Testing & Formal Transport Invariants**
  - Property-based testing (using native randomized generators) asserting:
    * ACKs can never acknowledge unsent sequence numbers.
    * Sequence progression is strictly monotonic modulo $2^{16}$.
    * Pending retransmission bytes never exceed configured capacity.
    * Duplicate packets are strictly idempotent.

---

## 4. Production Readiness Scorecard

To achieve full production sign-off, `eidolon` must satisfy all gates in this scorecard:

| Architectural Area | Priority | Evaluation Gate | Target Metric |
| :--- | :--- | :--- | :--- |
| **Core Simulation** | Completed | Deterministic 20 Hz tick loop | <5ms p99 tick duration (50ms budget) |
| **Spatial & AoI** | Completed | Flash mob saturation query | <10µs for 500-entity cluster |
| **Wire Egress** | Completed | L3/L4 Wire footprint | <1.2 KB/s average per client |
| **Session Security** | Completed | Cryptographic handshake & replay | 0 unauthenticated packets processed |
| **Anti-DoS Fencing** | Completed | Malicious client rate/memory bounds | <0.01% tick CPU per malicious client |
| **Authority Fencing** | Completed | Stale session & reconnect rejection | 0 stale state overwrites under network split |
| **Persistence (WAL)** | Completed | Crash consistency & dupe prevention | 0 lost transactions / 0 duplicated items |
| **State Reconstruction**| Completed | Disposable node recovery | 100% state restored after SIGKILL |
| **End-to-End Flow** | P1 | Overload backpressure | 0 unbounded queues; graceful shedding |
| **Distributed Chaos** | P1 | Multi-server 100k CCU harness | 100% entity accounting across zone seams |
| **Network Impairment** | P1 | Compound 5% loss + 150ms jitter | 0 desyncs; deterministic dead reckoning |
| **Soak Stability** | P1 | 7-day continuous execution | 0 RSS memory growth; 0 tick latency drift |
| **Rolling Upgrades** | P1 | Mixed-version cluster deployment | 0 client disconnects during version migration|
| **Transport Invariants**| P2 | Formal property-based assertions | 100% mathematical invariant verification |

---

<div align="center">
  <sub>eidolon Architecture Roadmap • Maintained by <a href="https://github.com/mikev-lab">Michael Valdez (mikev-lab)</a></sub>
</div>
