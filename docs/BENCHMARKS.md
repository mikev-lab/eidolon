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

Measured over 200 consecutive ticks (10.0 seconds of continuous 20 Hz simulation) with 2,000 synthetic bot clients communicating over UDP loopback sockets:

| Metric | Target Specification | Empirical Result | Margin of Compliance |
| :--- | :--- | :--- | :--- |
| **Average Wire Bandwidth** | < 1,200 B/s (1.2 KB/s) | **460.00 B/s (0.45 KB/s)** | **61.7% under budget** |
| **Total Datagrams Transmitted** | Bounded | **9,200,000 bytes (8.77 MB)** | Verified |
| **Authoritative Simulation Cadence** | 20 Hz (50ms interval) | **Locked at 20 Hz (+/- 5ms)** | Target-instant pacing |
| **Zero Entity Loss Invariant** | 100% entity accounting | **2,000 / 2,000 entities intact** | 0 entities lost |
| **Seam Boundary Handoffs** | In-memory atomic migration | **21,875 seam migrations** | 0 duplicate entities |
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
