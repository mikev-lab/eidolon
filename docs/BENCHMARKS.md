# eidolon Verified Performance Benchmarks & Wire Metrics

**Status:** Production Engine Complete  
**Classification:** Public Engineering Specification  

---

## 1. Verified Microbenchmark Measurements (Release Profile)

Measurements collected via the native nanosecond microbenchmark suite (`crates/eidolon-server/tests/microbenchmarks.rs`) on Apple Silicon / NEON hardware:

| Subsystem / Operation | Iterations | Latency (ns/op) | Throughput (ops/sec) | Verification Status |
| :--- | :--- | :--- | :--- | :--- |
| **`Fixed64::saturating_mul`** | 500,000 | **1.00 ns** | **998,003,992** | PASSED (<100ns) |
| **`Fixed64::sqrt`** | 200,000 | **60.22 ns** | **16,605,378** | PASSED (<250ns) |
| **`Fixed64::clamp` (Branchless)** | 500,000 | **0.69 ns** | **1,454,194,333** | PASSED (<50ns) |
| **`Vec3Fix::distance_squared` (Scalar)** | 500,000 | **2.12 ns** | **471,198,021** | PASSED (<50ns) |
| **`Vec3Fix::batch_distance_squared_4x` (SIMD)**| 200,000 | **7.62 ns** (1.9ns/lane) | **131,219,303** | PASSED (<100ns) |
| **`QuantizedCellCoord::quantize`** | 500,000 | **1.06 ns** | **945,626,478** | PASSED (<50ns) |
| **`QuantizedCellCoord::pack (7B)`** | 500,000 | **0.75 ns** | **1,338,985,638** | PASSED (<50ns) |
| **`QuantizedCellCoord::unpack (7B)`** | 500,000 | **0.84 ns** | **1,187,648,456** | PASSED (<50ns) |
| **`BitWriter::write_bits` (Arbitrary)** | 200,000 | **34.33 ns** | **29,126,564** | PASSED (<200ns) |
| **`BitReader::read_bits`** | 200,000 | **10.02 ns** | **99,765,153** | PASSED (<200ns) |
| **`SpatialHashGrid::query_radius_squared`** | 100,000 | **4,989.55 ns** | **200,419** | PASSED (<5us) |
| **`SpatialHashGrid::query_radius_batched`** | 100,000 | **4,969.23 ns** | **201,238** | PASSED (<5us) |
| **`kinematics::extrapolate`** | 500,000 | **9.00 ns** | **111,083,340** | PASSED (<50ns) |
| **`kinematics::should_dispatch_update`** | 500,000 | **4.53 ns** | **220,807,387** | PASSED (<50ns) |
| **`SpscPacketQueue::try_push + try_pop`** | 500,000 | **62.43 ns** | **16,018,218** | PASSED (<100ns) |

---

## 2. 2,000 CCU Bot Load Simulation Results

Measured over 200 consecutive ticks (10.0 seconds of continuous 20 Hz simulation) with 2,000 synthetic bot clients in a multi-zone world topology and 100 active observer clients communicating over 4 multiplexed UDP loopback sockets with full 3D spatial queries, visibility reconciliation, and tiered AoI replication:

| Metric | Target Specification | Empirical Result | Margin of Compliance |
| :--- | :--- | :--- | :--- |
| **Server Replication Egress (L7 Payload)** | < 1,228.8 B/s (1.2 KB/s) | **766.51 B/s (0.75 KB/s)** | 37.6% budget headroom |
| **Server Replication Egress (L3/L4 Wire)** | < 1,228.8 B/s (1.2 KB/s) | **1,046.51 B/s (1.02 KB/s)** | Conforms to wire budget (includes 28B IP/UDP framing) |
| **Client Input Ingress (L7 Payload)** | < 1,228.8 B/s (1.2 KB/s) | **15.43 B/s (0.02 KB/s)** | 98.7% under budget (Dead Reckoning) |
| **Client Input Ingress (L3/L4 Wire)** | < 1,228.8 B/s (1.2 KB/s) | **30.32 B/s (0.03 KB/s)** | 97.5% under budget (includes 28B IP/UDP framing) |
| **Reconstructible Replication Accuracy** | < 2.0 mm tolerance | **0.977 mm (< 1.0 mm)** | Sub-millimeter global coordinate reconstruction |
| **Total Server Egress Transmitted (L7)** | Bounded | **766,514 bytes (0.73 MB)** | Full tiered replication datagrams |
| **Total Server Egress Transmitted (L3/L4)**| Bounded | **1,046,514 bytes (1.00 MB)**| Includes 28-byte IP/UDP headers |
| **Authoritative Simulation Cadence** | 20 Hz (50ms interval) | **Locked at 20 Hz (+/- 5.0ms)** | Target-instant pacing |
| **Zero Entity Loss Invariant** | 100% entity accounting | **2,000 / 2,000 entities intact** | 0 entities lost across zones |
| **Seam Boundary Handoffs** | In-memory atomic migration | **1,580 seam migrations** | 0 duplicate entities |
| **Active Load Shedding Level** | `LoadSheddingLevel::None` | **Level: None (Zero overruns)** | 100% time budget met |
| **Heap Memory Allocation in Loop** | Zero allocations | **0 bytes allocated** | Pre-allocated ring buffers |
| **Crash Freedom & Error Handling** | Zero panics | **Zero panics, zero aborts** | 100% safe execution |

---

## 3. Sustained Load & Zero Memory Leak Audit (100,000 Ticks)

Measured across 100,000 continuous simulation ticks:
- **100 Active Entities:** Oscillating continuously across 16m zone boundaries.
- **Scale-to-Zero Co-op Dungeons:** 200 ephemeral dungeon rooms allocated, fought, and deallocated upon player departure.
- **Result:**
  * Total in-memory entity migrations: **>1,000 migrations**.
  * Total dungeon lifecycles completed: **>50 cycles**.
  * Active room count at completion: **0 rooms ($0 active compute)**.
  * Memory utilization: **Completely flat with zero memory leaks**.

---

## 4. Capacity Headroom & Latency Percentiles (Baseline 2,000 CCU)

Collected via `crates/eidolon-server/tests/extreme_stress_benchmarks.rs` across 2,000 active synthetic bots and 100 full-stack observer clients in release profile:

| Metric / Phase | Latency (Release Profile) | Latency (Debug Profile) | Allocated Budget | Measured Headroom |
| :--- | :--- | :--- | :--- | :--- |
| **Minimum Tick Duration** | **2.67 ms (2,666 µs)** | 11.50 ms (11,497 µs) | 50.00 ms (20 Hz) | **94.7% Headroom** |
| **Mean Tick Duration** | **3.08 ms (3,079 µs)** | 12.54 ms (12,542 µs) | 50.00 ms (20 Hz) | **93.8% Headroom** |
| **Median (p50) Tick Duration** | **3.06 ms (3,061 µs)** | 12.28 ms (12,275 µs) | 50.00 ms (20 Hz) | **93.9% Headroom** |
| **75th Percentile (p75)** | **3.23 ms (3,226 µs)** | 12.62 ms (12,617 µs) | 50.00 ms (20 Hz) | **93.5% Headroom** |
| **90th Percentile (p90)** | **3.35 ms (3,349 µs)** | 13.17 ms (13,174 µs) | 50.00 ms (20 Hz) | **93.3% Headroom** |
| **95th Percentile (p95)** | **3.89 ms (3,888 µs)** | 13.70 ms (13,699 µs) | 50.00 ms (20 Hz) | **92.2% Headroom** |
| **99th Percentile (p99)** | **4.75 ms (4,755 µs)** | 18.60 ms (18,602 µs) | 50.00 ms (20 Hz) | **90.5% Headroom** |
| **Maximum Tick Duration** | **5.15 ms (5,147 µs)** | 25.92 ms (25,916 µs) | 50.00 ms (20 Hz) | **89.7% Headroom** |

### Phase Breakdown at p99 Latency (Release Profile)
- **Phase 1: Ingress & Packet Drain:** **1.27 ms (1,272 µs)** (100+ client input packets received and parsed)
- **Phase 2: World Simulation & Seams:** **0.02 ms (16 µs)** (movement updates and atomic boundary handoffs)
- **Phase 3: 3D Spatial Queries & AoI:** **3.23 ms (3,226 µs)** (100 full 3D spatial queries + visibility diffing + packet packing)
- **Phase 4: Egress Flush:** **0.51 ms (513 µs)** (dispatched replication datagrams to UDP network queues)
- **Active Load Shedding:** `LoadSheddingLevel::None` (Zero tick overruns, 100% time budget met)

---

## 5. Extreme Density Stress Testing (75th to 99th Percentile Cluster Scaling)

In real MMO gameplay, players naturally cluster into high-density gatherings (world bosses, trade hubs, bridge skirmishes). The engine enforces strict per-client bandwidth clamping and sub-millisecond spatial queries across density extremes:

| Scenario | Entity Density | Spatial Query Latency | Visibility Reconcile Latency | Wire Egress per Observer | Wire Budget Status |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **50th Percentile (Ambient)** | 10 entities in AoI | **0.33 µs** | **0.25 µs** | **1,170.00 B/s (1.14 KB/s)** | Conforms (<1.2 KB/s) |
| **75th Percentile (Hub Skirmish)** | 50 entities in AoI | **0.70 µs** | **1.17 µs** | **1,170.00 B/s (1.14 KB/s)** | Conforms (<1.2 KB/s) |
| **90th Percentile (Chokepoint)** | 100 entities in AoI | **1.47 µs** | **2.30 µs** | **1,170.00 B/s (1.14 KB/s)** | Conforms (<1.2 KB/s) |
| **95th Percentile (Major Raid)** | 250 entities in AoI | **4.74 µs** | **10.76 µs** | **1,170.00 B/s (1.14 KB/s)** | Conforms (<1.2 KB/s) |
| **99th Percentile (Flash Mob)** | 500 entities in single cell | **7.33 µs** | **25.52 µs** | **1,170.00 B/s (1.14 KB/s)** | Conforms (<1.2 KB/s) |

> **The Architectural Takeaway:** Even with 500 entities packed into a single 64-meter cell (99th percentile extreme crowd density), spatial queries execute in **under 8 microseconds**, and client egress remains strictly clamped at **1,170.00 B/s**, completely preventing network packet storms or client buffer bloat.

---

## 6. Real-World Headcount Economics & Cloud Egress Savings

For comprehensive financial models, cloud provider rate comparisons (AWS, GCP, Azure, bare-metal), and the End-of-Service (EoS) perpetual maintenance framework, see the authoritative whitepaper:  
👉 **[`docs/COST_ANALYSIS.md`](./COST_ANALYSIS.md)**

### Annual Cost Comparison Summary (at $0.07/GB Blended Egress)
| Headcount Tier | Unoptimized Baseline (20 KB/s) | **eidolon Authoritative Wire (1.02 KB/s)** | Annual Studio Savings |
| :--- | :--- | :--- | :--- |
| **5 Players (Dev / Solo / EoS)** | $142.44 / year | **$7.32 / year (100% Free on GCP)** | Free Tier Qualified |
| **1,000 CCU (Indie / Private)** | $28,488 / year | **$1,452 / year** | **$27,036 / year (94.9%)** |
| **10,000 CCU (Successful Indie)** | $284,856 / year | **$14,532 / year** | **$270,324 / year (94.9%)** |
| **100,000 CCU (Top Steam Title)** | $2,848,572 / year | **$145,272 / year** | **$2,703,300 / year (94.9%)** |
| **1,000,000 CCU (Global Hit)** | $28,485,732 / year | **$1,452,768 / year** | **$27,032,964 / year (94.9%)** |
| **5,000,000 CCU (Peak Scale)** | $142,428,672 / year | **$7,263,864 / year** | **$135,164,808 / year (94.9%)** |
