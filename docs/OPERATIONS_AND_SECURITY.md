# eidolon Production Operations, Observability & Security Architecture

**Classification:** Engineering Operations Specification & Threat Model  
**Target Engine:** `eidolon` Authoritative MMO Server  
**Audience:** Principal Systems Engineers, Site Reliability Engineers (SRE), and Security Auditors  

---

## Executive Summary

Engineering a high-performance authoritative game server requires more than low-latency simulation loops; it demands operational resilience, adversarial defense, and backward-compatible protocol evolution.

This document establishes the production operating architecture for `eidolon`:
1. **Adversarial Threat Model & Defenses:** Mitigations against UDP amplification, sequence spoofing, memory exhaustion, and spatial CPU denial-of-service.
2. **Observability & Health Telemetry:** Phase-level microsecond tick metrics, queue backpressure telemetry, and Agones Kubernetes lifecycle integration.
3. **Protocol Evolution & Backward Compatibility:** Versioning, header framing invariants, and forward-compatible bitmask expansion.
4. **Disaster Recovery & Cluster Failover:** Transactional zone migration rollback, node eviction handling, and cold-state account persistence.

---

## 1. Adversarial Threat Model & Network Defenses

Operating an authoritative game server over UDP exposes the daemon to the hostile public internet. `eidolon` enforces strict cryptographic and protocol boundaries:

### 1.1 UDP Amplification & Reflection Defense
- **The Threat:** Attackers spoof victim IP addresses to trigger large server datagram responses, using the game server as a Denial of Service amplifier.
- **Engine Invariant:**
  * Unauthenticated handshake packets (heartbeats, connection handshakes) must never generate responses larger than the incoming datagram.
  * Ingress client input packets average **17 to 34 bytes**; server heartbeat replies are strictly clamped to **12 bytes** (12-byte header with 0-byte payload).
  * Amplification factor is strictly **< 1.0x** for all unauthenticated traffic.

### 1.2 Packet Replay & Sequence Spoofing
- **The Threat:** Adversaries capture valid movement or combat packets and replay them to desynchronize server state or duplicate actions.
- **Engine Invariant:**
  * Every packet carries a 16-bit sequence number evaluated against a circular sliding window using wrapping arithmetic:
    $$\text{diff} = \text{packet\_seq.wrapping\_sub}(\text{expected\_seq})$$
  * Packets with sequence distances $\ge 32,768$ are classified as old and discarded.
  * Unreliable sequenced channels (`ChannelType::UnreliableSequenced`) strictly reject out-of-order and duplicate packets.
  * Reliable channels (`ChannelType::ReliableOrdered`) process remote ACKs against a 32-bit bitfield, ignoring duplicate ACKs and preventing replay-induced state mutations.

### 1.3 Memory Exhaustion & Buffer Overrun Mitigation
- **The Threat:** Malicious clients stream corrupt variable-length integers (varints) or claiming payloads larger than actual buffer bounds to trigger buffer overruns or heap exhaustion.
- **Engine Invariant:**
  * Zero dynamic allocations during untrusted packet ingestion: all incoming datagrams are parsed using zero-copy slice views (`PacketView::from_bytes`).
  * Array indexing (`slice[i]`) is forbidden; all offsets use bounded methods (`slice.get()`).
  * Varint decoders (`BitReader::read_varint`) enforce a hard 5-byte continuation limit, returning `NetError::MalformedPayload` if continuation bits exceed 32-bit limits (thwarting continuation bit attacks).
  * Hard-bounded SPSC packet rings (`SpscPacketQueue<CAP>`): queue limits are fixed at process startup. Slow clients or packet floods trigger deterministic drops rather than heap growth.

### 1.4 Spatial Query Denial-of-Service (CPU Exhaustion)
- **The Threat:** Adversaries herd hundreds of entities into a single coordinate to force $O(N^2)$ neighbor searches, exhausting the server tick budget.
- **Engine Invariant:**
  * Spatial partitioning uses flat spatial hash cell buckets with pre-allocated intrusive slot-maps.
  * 4-wide SIMD vector filtering (`Vec3Fix::batch_distance_squared_4x`) evaluates candidate proximity in parallel register lanes.
  * Spatial query results return bounded structures (`SpatialQueryResult`) with explicit truncation counters, preventing unbounded array traversals during extreme clustering.

---

## 2. Observability, Telemetry & SRE Runbook

### 2.1 Microsecond Phase Latency Telemetry
The authoritative simulation loop divides each 50ms tick into 4 distinct instrumented phases:

```
┌────────────────────────────────────────────────────────────────────────┐
│                        50.0 ms Simulation Tick                         │
├───────────────┬───────────────────┬───────────────────┬────────────────┤
│    Phase 1    │      Phase 2      │      Phase 3      │    Phase 4     │
│ Ingress Drain │     World Sim     │   Spatial & AoI   │  Egress Flush  │
│  (~1.27 ms)   │    (~0.02 ms)     │    (~3.23 ms)     │   (~0.51 ms)   │
└───────────────┴───────────────────┴───────────────────┴────────────────┘
▲                                                                        ▲
│                                                                        │
└─────────────────────── 44.97 ms Free Headroom ─────────────────────────┘
```

Production Prometheus/OpenTelemetry gauge definitions:
- `eidolon_tick_duration_micros`: Total tick duration (p50, p95, p99). Alert threshold: `p99 > 35,000 µs`.
- `eidolon_phase_ingress_micros`: Network worker drain latency. Alert threshold: `> 5,000 µs`.
- `eidolon_phase_sim_micros`: Entity movement and boundary checks. Alert threshold: `> 10,000 µs`.
- `eidolon_phase_spatial_micros`: 3D spatial queries and AoI reconciliation. Alert threshold: `> 15,000 µs`.
- `eidolon_phase_egress_micros`: UDP egress queue dispatch. Alert threshold: `> 5,000 µs`.

### 2.2 Adaptive Load Shedding Telemetry
Monitored via `TickCoordinator::shedding_level()`:
- `Level 0 (None)`: Normal operation (10 Hz Immediate, 2 Hz Mid, Horizon events).
- `Level 1 (Mild)`: Triggered at $\ge 40\text{ms}$ (80% budget). Mid tier throttled to 1 Hz; horizon events deferred.
- `Level 2 (Aggressive)`: Triggered at $\ge 45\text{ms}$ (90% budget). Mid tier dropped; immediate combat prioritized at 10 Hz.
- `Watchdog Circuit Breaker`: Triggered at $\ge 49\text{ms}$ (98% budget). Non-essential simulation tasks aborted to guarantee 20 Hz tick cadence.
- SRE Alert: Any sustained transition to `Level 2` lasting $> 30\text{ seconds}$ triggers an automated Agones pod horizontal scale-out.

### 2.3 Agones Kubernetes Health Integration
`eidolon-server` integrates natively with Google Cloud Agones via direct non-blocking TCP IPC (`AgonesClient`):
- `GET /health` (every 2.0 seconds / 40 ticks): Maintains pod liveness. If the simulation loop hangs or panics, the heartbeat ceases and Kubernetes restarts the container within 6 seconds.
- `POST /ready`: Dispatched on startup after pre-allocating memory pools and establishing network bindings.
- `POST /allocate`: Dispatched when player sessions connect and zones transition to active gameplay.
- `POST /shutdown`: Dispatched during graceful node eviction to inform the cluster coordinator that memory was cleanly serialized.

---

## 3. Wire Protocol Evolution & Semantic Versioning

To support long-term production live-service operations without breaking existing game clients, `eidolon` enforces strict protocol versioning:

### 3.1 Packet Framing Schema
Every UDP datagram begins with a mandatory 12-byte header:

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|          Magic (0x4549)       |  Proto Ver (1)| Channel / Type|
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|        Sequence Number        |          ACK Number           |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                         ACK Bitfield                          |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

1. **Magic (`0x4549` / ASCII `EI`):** Validates that incoming datagrams are intended for the `eidolon` engine. Mismatched packets are dropped at the socket boundary without processing.
2. **Protocol Version (`u8`):** Authoritative wire protocol version (`PROTOCOL_VERSION = 1`). Server rejects packets with incompatible major protocol versions with typed `NetError::IncompatibleProtocolVersion`.
3. **Channel Type & Packet Type (`u8`):** High nibble encodes channel (Unreliable, Sequenced, Reliable); low nibble encodes packet type (Handshake, StateUpdate, Disconnect).

### 3.2 Backward-Compatible Transform Bitstream Expansion
The 7-byte kinematic transform packs coordinates, yaw, and flags:
- `Byte 0..1`: Cell-relative X coordinate (16 bits).
- `Byte 2..3`: Cell-relative Z coordinate (16 bits).
- `Byte 4`: Discrete yaw angle (8 bits / 256 divisions).
- `Byte 5`: Cell-relative Y coordinate low byte (8 bits).
- `Byte 6`: High 4 bits encode Y coordinate; low 4 bits encode state flags:
  * Bit 0 (`0x01`): Moving flag.
  * Bit 1 (`0x02`): Sprinting flag.
  * Bit 2 (`0x04`): Reserved for future state expansion.
  * Bit 3 (`0x08` / Wire Bit 7 `0x80`): Cell Anchor Flag (`FLAG_CELL_ANCHOR`).
- Forward Compatibility: Reserved flag bits allow future gameplay expansions (e.g. crouching, mounted, airborne) without altering the 7-byte wire footprint or breaking legacy prediction algorithms.

---

## 4. Disaster Recovery & Cluster Failover

In a distributed multi-zone cluster, node failures and network partitions are inevitable. `eidolon` provides built-in architectural resilience:

### 4.1 Transactional Zone Migration Rollback
When an entity crosses an open-world seam between neighboring zones:
1. The source zone emits a migration ticket via `WorldManager::tick_entity_movement`.
2. Target zone attempts entity insertion.
3. If target zone insertion fails (capacity exhaustion, cell boundary corruption, or network timeout), `WorldManager::rollback_entity_migration` executes:
   * Restores entity position and velocity to the source zone.
   * Clears in-flight migration status.
   * Preserves zero entity loss invariant (100% accounting).

### 4.2 Graceful Agones Pod Eviction & Drainage
When Kubernetes schedules node maintenance or autoscaling down-scaling:
1. Kubernetes sends `SIGTERM` to the `eidolon-server` pod.
2. The authoritative loop detects the signal and transitions state to `Draining`.
3. Client connections receive a reliable `PacketType::Disconnect` with a migration referral token.
4. Active player rosters, pity counters, and inventories serialize to cold binary snapshots (<256 bytes) via `eidolon-world::hibernation`.
5. Snapshots flush to S3 / Cloud Storage Coldline in $< 50\text{ ms}$.
6. Daemon notifies Agones via `POST /shutdown` and terminates with exit code 0.
