# eidolon Architecture Specification

**Status:** Production Engine Complete (All 7 Phases Implemented & Verified)  
**Classification:** Public Engineering Specification  

---

## 1. Domain and Architectural Mission

`eidolon` is an authoritative, high-concurrency, ultra-low-bandwidth (<1 KB/s design target) zoned and instanced MMO world server engine engineered in native Rust from first principles.

Most MMO backends fail economically because cloud network egress is billed between $0.05 and $0.12 per gigabyte. Transmitting 20 KB/s per player to 1,000,000 concurrent users (CCU) generates over $2.4M per month in bandwidth alone.

`eidolon` solves this through an architectural mandate:
1. **Sub-1.2 KB/s Wire Budget:** Target 0.4 to 0.8 KB/s average per client via intent-based dead reckoning, 16-bit coordinate quantization, 1-byte yaw, dynamic Area of Interest (AoI) frequency tiers, and zero-copy flat serialization.
2. **Zero External Runtime Dependencies:** Everything is of native first-principles design in Rust using only `std` / `core`. Zero third-party ECS frameworks, zero black-box netcode crates, zero external bitpackers.
3. **Zero-Allocation Hot Tick Loop:** Zero heap allocations (`Vec::new()`, `String`, `Box`, `clone()`) inside the fixed-step simulation loop. All spatial structures and packet queues use pre-allocated buffers.
4. **Scale-to-Zero Compute for Live-Service & Gacha:** Ephemeral dungeon instances allocate in <50ms and reclaim 100% of compute immediately upon party departure, enabling games to run in perpetual maintenance mode for practically $0 to $5/month indefinitely.

---

## 2. High-Level Dataflow

```text
                      [ Client UDP Traffic ]
                                │
                                ▼
               ┌─────────────────────────────────┐
               │    Async Network I/O Worker     │  (std::net::UdpSocket)
               │          (eidolon-net)          │  Non-blocking Ingress / Egress
               └────────────────┬────────────────┘
                                │ SPSC Queue (Zero-Allocation)
                                ▼
               ┌─────────────────────────────────┐
               │   Authoritative Simulation Loop │  (20 Hz / 50ms Tick)
               │        (eidolon-server)         │  Strict Zero Allocation
               ├─────────────────────────────────┤
               │ 1. Drain Ingress Packets        │
               │ 2. Validate Inputs & Intent     │
               │ 3. Update Dead Reckoning FSM    │
               │ 4. Update Spatial Hash Grid     │
               │ 5. Calculate Tiered AoI Events  │
               │ 6. Quantize & Pack Outbox Diffs │
               └────────────────┬────────────────┘
                                │ SPSC Queue (Zero-Allocation)
                                ▼
               ┌─────────────────────────────────┐
               │    Async Network I/O Worker     │  (UDP Send Batching)
               │          (eidolon-net)          │  Quantized Bitstreams (<1 KB/s)
               └─────────────────────────────────┘
```

---

## 3. Subsystem Organization

`eidolon` is organized as a modular Rust Cargo workspace:

| Crate | Core Responsibilities |
| :--- | :--- |
| **`eidolon-core`** | 32.32 fixed-point linear algebra (`Fixed64`, `Vec3Fix`), coordinate quantization, 7-byte transform bitpacking, and deterministic dead reckoning kinematics. |
| **`eidolon-net`** | Register-width bitstream reader/writer (`BitReader`, `BitWriter`), 12-byte header packet framing, sequenced unreliable channels, ordered reliable channels, and circular ring buffers. |
| **`eidolon-spatial`** | Cache-conscious 2D/3D spatial hash grid with contiguous memory buffers, intrusive doubly-linked slot indexing, 4-wide SIMD batching, and 3-tier AoI scheduling. |
| **`eidolon-world`** | Persistent open-world zone management with 16-meter overlapping seams, atomic in-memory entity migration, ephemeral dungeon instance pools, gacha account cold hibernation, and companion patch negotiation. |
| **`eidolon-server`** | Headless production daemon, 20 Hz tick coordinator with monotonic target-instant pacing, non-blocking UDP worker, native Agones Kubernetes sidecar IPC, and 2,000 CCU load harness. |

---

## 4. Tick Coordinator & Target-Instant Pacing

The server simulation executes at a fixed 20 Hz frequency (50ms per tick). To eliminate cumulative OS timer drift caused by kernel scheduler wakeup latency, `TickCoordinator` uses monotonic target-instant pacing:

$$\text{target}_k = \text{start\_instant} + k \times \text{tick\_interval}$$

If an OS sleep overshoots by 2ms on one tick, subsequent ticks dynamically shorten their sleep intervals to absorb the drift, holding long-term simulation drift strictly to zero (+/- 1ms jitter).

### Adaptive Spatial Load Shedding
When CPU spikes occur:
- **Level 1 (>= 40ms / 80% budget):** Horizon tier events are dropped; Mid tier throttles to 1 Hz.
- **Level 2 (>= 45ms / 90% budget):** Mid and Horizon tiers are dropped entirely, protecting 10 Hz Immediate combat.
- **Watchdog Circuit Breaker (>= 49ms / 98% budget):** Hard circuit breaker aborting non-essential operations to preserve 20 Hz simulation cadence.
- **Hysteresis Recovery:** Requires 5 consecutive normal ticks to step down from Level 2 to Level 1, and 10 consecutive normal ticks to return to Normal.

---

## 5. Agones Kubernetes Orchestration

For cloud hosting on Kubernetes, `AgonesClient` provides direct native HTTP/1.1 sidecar communication over local TCP socket (`127.0.0.1:9358`) without external HTTP crates:
- `POST /ready`: Signals pod initialization complete and ready for player matchmaking.
- `POST /health`: Periodic heartbeat ping (every 2 seconds) maintaining pod liveness.
- `POST /allocate`: Transitions server to Allocated state when active sessions connect.
- `POST /shutdown`: Graceful pod termination notification on server drain.
- **Standalone Mode:** Degrades gracefully to an instant, zero-cost mock when running locally outside Kubernetes.
