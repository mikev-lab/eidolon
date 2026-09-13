# eidolon Spatial Partitioning & Area of Interest (AoI)

**Status:** Production Engine Complete  
**Classification:** Public Engineering Specification  

---

## 1. Cache-Conscious Spatial Hash Grid

The `eidolon-spatial` crate partitions entities across a 2D/3D grid of uniform cubic cells ($64\text{m} \times 32\text{m} \times 64\text{m}$).

### Structural Innovations
1. **Struct-of-Arrays (SoA) Contiguous Layout:** Flat, parallel arrays store entity positions, cell coordinates, and hash keys. Sequential memory layouts maximize CPU hardware L1/L2 prefetcher utilization.
2. **Doubly-Linked Intrusive Bucket Lists:** Entities in the same cell bucket form an intrusive doubly-linked list via `next_in_cell` and `prev_in_cell` arrays.
3. **Strict $O(1)$ Operations:** Entity insertion, removal, and intra-cell position updates execute in constant time $O(1)$ with zero runtime heap allocations.

---

## 2. 4-Wide SIMD Batched Range Queries

During spatial queries, checking distance to dozens of candidate entities sequentially causes CPU pipeline stalls. `SpatialHashGrid` provides `query_radius_squared_batched`:
- Batches candidate entity positions into 4-wide chunks (`[Vec3Fix; 4]`).
- Evaluates squared Euclidean distances across all 4 lanes simultaneously using `Vec3Fix::batch_distance_squared_4x`.
- Compiles directly to hardware vector instructions (NEON on ARM, AVX2 on x86_64) without `unsafe` code.

---

## 3. Dynamic 3-Tier Area of Interest (AoI) Scheduling

Entities replicate to observers according to spatial proximity tiers:

```text
       Observer (Origin)
           │
  [ < 10m: Immediate Tier ]      --> 10 Hz Frequency Scheduling
           │
  [ 10m - 50m: Mid Tier ]        --> 2 Hz Frequency Scheduling
           │
  [ > 50m: Horizon Tier ]        --> Event-Driven Scheduling (Enter/Exit only)
```

### Spatial Hysteresis Deadbands
To prevent rapid ping-pong oscillations when an entity hovers near a tier boundary:
- **Immediate -> Mid Promotion:** Triggers only when distance exceeds **11.0m** ($121\text{ m}^2$).
- **Mid -> Immediate Demotion:** Triggers only when distance drops below **9.0m** ($81\text{ m}^2$).
- **Mid -> Horizon Promotion:** Triggers only when distance exceeds **52.0m** ($2,704\text{ m}^2$).
- **Horizon -> Mid Demotion:** Triggers only when distance drops below **48.0m** ($2,304\text{ m}^2$).

---

## 4. Adaptive Load Shedding

When simulation tick execution times threaten the 50ms budget:
- **Level 1 (>= 40ms / 80% budget):** Mid-tier updates throttled to 1 Hz; Horizon tier events suppressed.
- **Level 2 (>= 45ms / 90% budget):** Mid and Horizon tiers dropped entirely, protecting Immediate combat.
- **Watchdog Circuit Breaker (>= 49ms / 98% budget):** Aborts non-essential work to protect the 20 Hz simulation tick.
- **Hysteresis Recovery:** Requires 5 consecutive normal ticks to step down from Level 2 to Level 1, and 10 consecutive normal ticks to return to Normal.
