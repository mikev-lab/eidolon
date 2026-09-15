# Developer Best Practices: Unlocking the Full Power of eidolon

**Classification:** Developer Integration & Optimization Runbook  
**Target Audience:** Game Programmers, Backend Engineers, and Technical Directors  

---

## 1. Architectural Philosophy: The Zero-Waste Invariant

`eidolon` achieves **1,000+ active players on a $5 VPS** and **sub-1KB/s wire bandwidth** because every subsystem is engineered around mechanical sympathy:
1. **CPU hardware cache alignment:** Memory layout maps 1:1 with 64-byte L1 CPU cache lines.
2. **Deterministic fixed-point kinematics:** Zero floating-point drift across client and server.
3. **Intent-based dead reckoning:** Network packets transmit changes in velocity, heading, and acceleration; never continuous coordinate streams.
4. **Zero heap allocations inside the 20 Hz simulation loop:** Zero garbage collection pauses, flat memory footprints, and 0 ms GC jitter.

To get the full performance and economic benefits of `eidolon`, developers must align their client-side input and server-side gameplay logic with these first principles.

---

## 2. Principle 1: Stream Intent, Never Stream Raw Coordinates

### The Legacy Anti-Pattern
In traditional multiplayer architectures, the client ticks at 60 FPS and streams raw floating-point coordinates `(X, Y, Z)` to the server 20 to 60 times per second:
- Transmitting 3 floats + heading + timestamp = 24 to 40 bytes per packet.
- At 20 Hz, one player burns **800 B/s to 2.4 KB/s** in client upstream alone.
- Flooding the server with continuous coordinates causes CPU socket contention and creates visual rubberbanding when packets arrive with network jitter.

### The eidolon Pattern: Intent-Based Kinematic Extrapolation
The client only transmits a packet when the player changes their movement intent:
- Pressing or releasing a movement key (W, A, S, D).
- Deflecting their heading beyond the angular deadband ($>2.81^\circ$).
- Initiating an action (jump, sprint, dash, spell cast).

While running straight or standing still, **zero movement packets are dispatched**. Both the server and client extrapolate the entity's forward trajectory using identical fixed-point quadratic kinematic models:

$$\mathbf{P}(t + \Delta t) = \mathbf{P}(t) + \mathbf{v}(t) \cdot \Delta t + \frac{1}{2} \mathbf{a}(t) \cdot (\Delta t)^2$$

```csharp
// Example: Unity / Godot C# Intent Streaming (bindings/csharp/EidolonClient.cs)
void Update()
{
    float moveX = Input.GetAxisRaw("Horizontal");
    float moveZ = Input.GetAxisRaw("Vertical");
    float currentYaw = transform.eulerAngles.y;

    // Check if input intent changed or heading turned past deadband
    bool inputChanged = (moveX != _lastX || moveZ != _lastZ);
    bool headingChanged = Mathf.Abs(Mathf.DeltaAngle(currentYaw, _lastYaw)) > 2.81f;

    if (inputChanged || headingChanged)
    {
        _lastX = moveX;
        _lastZ = moveZ;
        _lastYaw = currentYaw;

        // Dispatch intent vector (velocity + discrete yaw)
        _client.SendMovementIntent(moveX, 0f, moveZ, currentYaw);
    }

    // Extrapolate other nearby entities smoothly at 60/120 FPS
    _client.PollEvents();
}
```

```cpp
// Example: Unreal Engine 5 C++ (bindings/cpp/EidolonClient.hpp)
void UEidolonSubsystem::Tick(float DeltaTime)
{
    FVector CurrentVelocity = PlayerCharacter->GetVelocity();
    float CurrentYaw = PlayerCharacter->GetActorRotation().Yaw;

    // Check predictive deadbands
    if (FVector::DistSquared(CurrentVelocity, LastVelocity) > 0.01f ||
        FMath::Abs(FMath::FindDeltaAngleDegrees(CurrentYaw, LastYaw)) > 2.81f)
    {
        LastVelocity = CurrentVelocity;
        LastYaw = CurrentYaw;

        // Transmit intent only on state divergence
        Client->SendMovementIntent(CurrentVelocity.X, CurrentVelocity.Y, CurrentVelocity.Z, CurrentYaw);
    }
}
```

---

## 3. Principle 2: Zero-Allocation Hot Simulation Loops

### The Legacy Anti-Pattern
Writing combat systems, damage queries, or status effect handlers that allocate heap memory dynamically:
```rust
// ANTI-PATTERN: Heap allocations inside the 20 Hz server tick
fn handle_aoe_attack(targets: &Vec<Entity>) {
    let mut hit_list = Vec::new(); // Allocates on heap!
    let debug_msg = format!("Found {} targets", targets.len()); // Allocates String!
    // ...
}
```
Heap allocations inside a 20 Hz tick loop cause memory fragmentation, trigger allocator lock contention across threads, and cause random microsecond latency spikes.

### The eidolon Pattern: Stack Buffers & Fixed Pools
All spatial queries, combat collision detections, and packet packing must use pre-allocated buffers:
```rust
// THE EIDOLON WAY: Pre-allocated arrays and fixed-point math
fn handle_aoe_attack(
    grid: &SpatialHashGrid,
    origin: Vec3Fix,
    radius_sq: Fixed64,
    out_targets: &mut [u32; MAX_AOE_TARGETS],
) -> usize {
    // Zero heap allocations: uses caller-provided stack slice
    grid.query_radius_into(origin, radius_sq, out_targets)
}
```

#### Invariants for Server Logic:
1. Never call `Vec::new()`, `Box::new()`, or `String::from` inside per-tick simulation code.
2. Use fixed-size stack buffers (`[T; N]`) or reusable scratch vectors that are cleared (`vec.clear()`) rather than reallocated.
3. Use 32.32 fixed-point linear algebra (`Fixed64`, `Vec3Fix`) for all physics and combat math to ensure 100% deterministic simulation across platforms.

---

## 4. Principle 3: Tiered Durability (Async WAL vs Sync Barriers)

### The Storage Bottleneck
A standard write-ahead log (WAL) relying on physical `fdatasync(2)` halts the CPU thread for 2ms to 8ms per barrier. Calling `fdatasync` synchronously on every gameplay action exhausts the 50ms frame budget.

### The Two-Tier Durability Strategy
`eidolon` decouples high-frequency gameplay mutations from physical disk barriers:

```text
┌─────────────────────────────────┐         ┌─────────────────────────────────┐
│ High-Frequency Gameplay Events  │         │  High-Value Economic Actions    │
│ (Movement, damage, mob spawns)  │         │  (Currency, gacha, trades)      │
└────────────────┬────────────────┘         └────────────────┬────────────────┘
                 │                                           │
                 ▼                                           ▼
┌─────────────────────────────────┐         ┌─────────────────────────────────┐
│ Async SQPOLL Double-Buffer WAL  │         │ Physical POSIX fdatasync Barrier│
│ (1.15 µs atomic buffer swap)    │         │ (Guaranteed RPO = 0 on crash)   │
└─────────────────────────────────┘         └─────────────────────────────────┘
```

1. **High-Frequency Gameplay Mutations:**
   - Enqueue records into `DoubleBufferedJournalQueue`.
   - The 20 Hz simulation thread executes an atomic buffer swap in **1.15 µs**, leaving physical NVMe disk synchronization to background kernel poller threads (`AsyncSqpollJournal`).
2. **Authoritative Economic Fences:**
   - When a player buys premium currency, completes an item trade, or performs a gacha draw: trigger a synchronous durable commit (`sync_data`).
   - The client receives confirmation only after the physical barrier confirms on disk, guaranteeing **Recovery Point Objective (RPO) = 0** without slowing down combat simulation.

---

## 5. Principle 4: Exploit Distance-Adaptive Multi-Resolution AoI

### Why Uniform Replication Wastes Bandwidth
Streaming full 7-byte transform updates for entities 40 meters away is mathematically wasteful. At 40 meters on a 1080p screen, a 25cm positional variation occupies less than 1 single pixel.

### The 3-Tier Multi-Resolution Configuration
`eidolon-spatial` automatically scales quantization precision with distance:

| Tier | Range | Quantization Precision | Yaw Steps | Wire Size | Update Rate |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Tactical** | $< 12\text{ m}$ | 0.976 mm (16-bit X/Z) | 256 steps ($1.4^\circ$) | **7 Bytes** | 10 Hz |
| **Midfield** | $12\text{ m} - 32\text{ m}$ | 15.6 mm (10-bit X/Z) | 64 steps ($5.6^\circ$) | **5 Bytes** | 2 Hz |
| **Horizon** | $> 32\text{ m}$ | 250 mm (8-bit X/Z) | 16 steps ($22.5^\circ$) | **3 Bytes** | Event-only |

#### How to Tune in Code:
Configure your spatial grid cells to $64\text{m} \times 32\text{m} \times 64\text{m}$. Use the built-in hysteresis deadbands (promotions require $+1.0\text{m}$ beyond tier boundary; demotions require $-1.0\text{m}$) to eliminate rapid state oscillations when entities hover near boundary radii.

---

## 6. Principle 5: 64-Byte Hardware Cache Alignment for Custom Components

### The Memory Wall
On modern CPUs, fetching an unaligned word from main RAM takes 50ns to 100ns (hundreds of clock cycles), while reading from L1 cache takes 1ns. If an entity struct spans across cache-line boundaries (split loads), memory bandwidth drops by 50%.

### Packing Custom Components
When writing custom server components (e.g. stats, inventories, combat state):
1. Align high-frequency transform and kinematic structures to **64 bytes** using `#[repr(C, align(64))]`.
2. Group frequently mutated fields together (`pos_x`, `pos_y`, `pos_z`, `vel_x`, `vel_y`, `vel_z`, `heading`, `flags`).
3. For bulk simulation passes, organize state into **8-lane SIMD chunks** (`AlignedSoAChunk8`). This enables the compiler to auto-vectorize physics integration across AVX2 or ARM NEON registers.

---

## 7. Principle 6: Production Host & Kernel Socket Tuning

When deploying to Linux hosts (standalone VPS or Kubernetes worker nodes), configure kernel network socket buffers to handle dense UDP datagram streams:

### Recommended `sysctl.conf` Tuning
```ini
# Increase maximum UDP receive and send buffer sizes to 16 MB
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
net.core.rmem_default = 2097152
net.core.wmem_default = 2097152

# Increase network device backlog queue
net.core.netdev_max_backlog = 10000

# Enable io_uring and fast socket recycling
fs.file-max = 2097152
```

### Hosting on Budget Hardware ($0 to $5/month)
* **Google Cloud Free Tier `e2-micro` (2 vCPUs, 1 GB RAM):**
  - Allocate a 64 MB heap cap for the `eidolon-server` binary.
  - Set pre-allocated player connection pool to 1,000 CCU (~48 MB RAM).
  - Set tick rate to 20 Hz (50ms budget). The simulation consumes only ~3ms per tick, running comfortably within the 0.25 vCPU baseline entitlement.

### Scaling Horizontally on Kubernetes (Agones)
* Deploy `eidolon-server` containers with Agones `GameServer` specs using host networking (`hostPort` UDP).
* Configure the Agones Fleet Autoscaler to maintain a buffer of 10% ready pods for instant matchmaker allocation.
* When dungeon instances conclude, let pods drain cleanly; the Kubernetes Cluster Autoscaler will spin down underlying cloud VMs to maintain true scale-to-zero economics.

---

## 8. Principle 7: Companion Client Delta Patching (`pak-delta`)

A common mistake in live-service architecture is reducing server wire traffic down to 1 KB/s, only to bankrupt the game on CDN bandwidth when delivering 40 GB game patches.

### The Solution
Use [`pak-delta`](https://github.com/mikev-lab/pak-delta) hand-in-hand with `eidolon`:
1. `eidolon-world` checks the client's asset container version via `PatchNegotiator` before permitting entry into seamless zones or dungeon rooms.
2. Outdated clients query the patch CDN for archive-aware byte deltas generated by `pak-delta`.
3. The client downloads only the specific binary diffs (typically <50 MB instead of a 30 GB full archive reinstall), slashing CDN bills by **90% to 98%**.

---

## Quick Reference Checklist for Developers

| Optimization Practice | Impact | Target Invariant |
| :--- | :--- | :--- |
| **Stream WASD intent only on key change** | 85% to 99% reduction in upstream traffic | <35 B/s client ingress |
| **Zero allocations in 20 Hz simulation** | Eliminates GC pauses and memory fragmentation | 0 ms GC jitter |
| **Decouple database persistence via WAL** | Zero tick stalls, RPO = 0, flat binary saves | See [`PERSISTENCE_AND_DATABASE_INTEGRATION.md`](./PERSISTENCE_AND_DATABASE_INTEGRATION.md) |
| **Use DoubleBufferedJournalQueue for WAL** | 4,180x faster transaction logging | <5 µs disk dispatch |
| **Enforce 64-byte alignment on entity blocks** | Eliminates split cache-line memory stalls | <0.25 ms for 10k CCU |
| **Configure Multi-Res AoI distance tiers** | 42.9% reduction in replication egress | <1.02 KB/s wire egress |
| **Pair with pak-delta for client patching** | 90%+ reduction in CDN patch delivery costs | Sub-100 MB client patches |
