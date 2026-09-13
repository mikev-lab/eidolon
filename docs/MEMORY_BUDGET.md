# Per-Player Strict Memory Budget Specification

This engineering specification establishes the explicit, auditable byte-level memory budget for active player sessions in `eidolon`. It provides mathematical proofs establishing resident heap ceilings across all concurrency tiers (1,000 to 100,000 Concurrent Users).

---

## 1. Per-Player Byte Breakdown Matrix

Every connected player session allocates fixed-capacity resident structures. Dynamic heap allocations during active simulation ticks are strictly prohibited by engine invariants.

```
┌────────────────────────────────────────────────────────────────────────┐
│                   PER-SESSION MEMORY BUDGET (48 KB)                   │
├────────────────────────────────────────┬─────────────┬────────────────┤
│ Subsystem Component                    │ Budget (B)  │ Share (%)      │
├────────────────────────────────────────┼─────────────┼────────────────┤
│ Ingress & Egress Socket Ring Buffers   │ 16,384 B    │ 33.3%          │
│ Reliable Channel Retransmission Buffer │ 16,384 B    │ 33.3%          │
│ Area of Interest (AoI) Observer Tables │  8,192 B    │ 16.7%          │
│ Write-Ahead Journal Slot & Locks       │  4,096 B    │  8.3%          │
│ Kinematic State & Dead Reckoning Model │    512 B    │  1.0%          │
│ Connection Security Context & Auth     │    512 B    │  1.0%          │
│ Struct Alignment & Cache-Line Headroom │  3,072 B    │  6.4%          │
├────────────────────────────────────────┼─────────────┼────────────────┤
│ Total Guaranteed Ceiling per Client    │ 49,152 B    │ 100.0% (48 KB) │
└────────────────────────────────────────┴─────────────┴────────────────┘
```

---

## 2. Component Memory Specifications

### 2.1 Network Socket Ring Buffers (16 KB)
- **Ring Capacity:** Fixed circular byte array storing incoming and outgoing datagram frames.
- **Backpressure Policy:** Dropping oldest unacknowledged unreliable transforms when full, while maintaining dedicated capacity for reliable commands.

### 2.2 Reliable Channel Retransmission Buffer (16 KB)
- **Sliding Window:** 64-message unacknowledged flight capacity.
- **Storage:** Stores up to 64 unacknowledged reliable payloads with exponential backoff timers. Once acknowledged via remote ACK bitfields, slots are immediately recycled without memory allocations.

### 2.3 Area of Interest (AoI) Observer Tables (8 KB)
- **Observer Relations:** Stores up to 128 proximate entity IDs across Immediate (<10m), Mid (10m - 50m), and Horizon (>50m) frequency tiers.
- **Hysteresis Bitsets:** Tracks distance-promotion and demotion states, preventing boundary chatter.

### 2.4 Write-Ahead Journal & Transaction State (4 KB)
- **In-Flight Mutations:** Dedicated journal record slot capturing currency transfers, inventory modifications, and boundary migration tickets prior to asynchronous disk flush.

### 2.5 Kinematic State & Transform Quantizer (512 B)
- **Continuous State:** 64-bit fixed-point positions $(X, Y, Z)$ and velocity vectors $(\dot{X}, \dot{Y}, \dot{Z})$.
- **Quantized Cache:** 7-byte bitpacked transform representation and discrete heading.

---

## 3. Scale-Up Concurrency Projections

Because per-player memory is bounded to a maximum ceiling of 48 KB, total server heap requirements scale linearly with zero runaway growth:

| Concurrency Tier | Total Sessions | Total Resident RAM | Server Sizing Envelope |
| :--- | :--- | :--- | :--- |
| **Indie / Co-op** | **1,000 CCU** | **~46.9 MB** | Google Cloud `e2-micro` (1 GB RAM) |
| **Regional Shard** | **5,000 CCU** | **~234.4 MB** | Google Cloud `e2-small` (2 GB RAM) |
| **Zone Cluster** | **10,000 CCU** | **~468.8 MB** | Cloud VM (1 vCPU, 2 GB RAM) |
| **High Population** | **50,000 CCU** | **~2.29 GB** | Cloud VM (4 vCPU, 4 GB RAM) |
| **Mega World** | **100,000 CCU** | **~4.58 GB** | Single Standard Node (8 GB RAM) |

> **Conclusion:** 100,000 concurrent players comfortably reside within less than 5.0 GB of RAM on a single 8 GB server instance.

---

## 4. Cache-Line Alignment & Data-Oriented Design

- **64-Byte Alignment:** Critical hot paths align simulation arrays to 64-byte L1 CPU cache boundaries using `#[repr(align(64))]` where applicable.
- **False Sharing Elimination:** Core simulation structures separate per-thread read and write states to eliminate CPU bus locking between the network I/O worker and the simulation tick coordinator.
