# Disaster Recovery, RPO/RTO Targets, and Crash Resilience Specification

## 1. Overview and Durability Guarantees

In high-concurrency multiplayer architectures, unexpected node failures, operating system panics, hardware faults, and network partitions are inevitable. `eidolon` enforces a mathematically proven, tiered recovery model guaranteeing zero data loss for economic transactions while keeping recovery times well within real-time MMO operational tolerances.

---

## 2. Quantitative Recovery Targets (RPO & RTO)

### 2.1 Recovery Point Objective (RPO)

| State Category | Maximum Allowable Data Loss (RPO) | Physical Enforcement Mechanism |
| :--- | :--- | :--- |
| **Acknowledged Economic Transactions** (Gacha, Currencies, Trades) | **RPO = 0 (Zero Data Loss)** | Synchronous POSIX `fdatasync` (`DurableFileJournal::append_and_sync`) executed before emitting client acknowledgment. |
| **Zone World State & Checkpoints** | **RPO <= 50ms (1 Tick)** | Periodic in-memory snapshotting combined with continuous WAL ring buffer flushes. |
| **Transient Kinematics** (Player Position, Velocity, Yaw) | **RPO <= 50ms (1 Tick)** | Intent-based dead reckoning extrapolation reconciles position upon client reconnection. |
| **Dungeon Instances** (Ephemeral Co-op Rooms) | **RPO = 0 for Rewards, Ephemeral for Mob Placement** | Instance completion events are logged durably; room geometry cleans up on exit. |

### 2.2 Recovery Time Objective (RTO)

| Failure Scenario | Target Recovery Time (RTO) | Automated Recovery Procedure |
| :--- | :--- | :--- |
| **Single Process Sudden Termination** (`SIGKILL`, OOM) | **RTO < 2.0 seconds** | Process restarts, loads baseline `ZoneCheckpoint`, and sequentially replays committed records from `DurableFileJournal`. |
| **Kubernetes Pod Eviction / Node Hardware Failure** | **RTO < 5.0 seconds** | Agones detects unhealthy game server pod, provisions replacement container on available node, and rebinds persistent volume. |
| **Cross-Zone Network Partition** | **RTO < 100ms** | Monotonic session generation fences immediately isolate partitioned node; lease timeout reclaims entity authority. |
| **Regional Data Center Disaster** | **RTO < 30.0 seconds** | Cold hibernation accounts hydrated from replicated cloud storage into secondary cluster. |

---

## 3. Physical Durability Contract (RPO = 0 Proof)

### 3.1 The Synchronous Commit Boundary

The critical invariant of `eidolon`'s durability contract is the order of execution between disk synchronization and client acknowledgment:

```text
Client Request  ───> [ Authoritative Simulation ]
                            │
                            ▼
                     [ In-Memory Mutation ]
                            │
                            ▼
                     [ Append to WAL Buffer ]
                            │
                            ▼
                     [ POSIX fdatasync() ]  <─── PHYSICAL BARRIER (Disk Flush)
                            │
                     ┌──────┴──────┐
             Success │             │ Failure / Crash
                     ▼             ▼
              [ Send ACK ]   [ Client Timeout ]
                                   │
                                   ▼
                             [ Re-transmit ]
```

1. **Before `fdatasync`:** If a `SIGKILL` or hardware power failure occurs before `fdatasync` completes, the transaction was never committed. The client never receives an acknowledgment packet. Upon reconnecting, the client re-submits the idempotently-deduplicated request.
2. **After `fdatasync`:** If a failure occurs immediately after `fdatasync` returns, the transaction bytes reside on non-volatile physical media. Upon reboot, `recover_from_journal` replays the transaction, restoring state completely.

### 3.2 Resilience to Truncated Trailing Writes

When a process is abruptly terminated mid-write, partial bytes may be left at the end of the journal file. `DurableFileJournal::recover_from_journal` handles this gracefully:
- Every record is preceded by an 8-byte frame header: 4-byte Magic (`0xE1D0_4A52`) and 4-byte record length.
- If EOF is reached before reading the full declared record length, recovery terminates cleanly at the last valid record boundary.
- If the Adler-32 / CRC checksum fails, recovery terminates cleanly without panicking.
- Partial trailing bytes are discarded, preventing corrupted state from entering the simulation loop.

---

## 4. Disaster Recovery Scenarios & Runbooks

### 4.1 Scenario A: Worker Node `SIGKILL` or Kernel Panic
1. **Detection:** SRE health probes or Agones ping monitoring misses 3 consecutive heartbeats (150ms).
2. **Action:** Supervisor or Kubernetes restarts the `eidolon-server` process.
3. **Execution:**
   - Server reads `checkpoint_<zone_id>.bin`.
   - Server streams `journal_<zone_id>.wal` sequentially up to last valid LSN.
   - Server binds UDP socket and resumes 20 Hz simulation loop.
4. **Verification:** Total elapsed downtime measured under 2 seconds.

### 4.2 Scenario B: Cross-Zone Partition and Reconnection
1. **Detection:** Inter-server UDP socket experiences >500ms packet loss.
2. **Action:** Partition detector triggers `BoundaryLease` timeout.
3. **Fencing:** Any stale mutation packets bearing old generation tokens are rejected with `WorldError::StaleSessionMutation`.
4. **Reconciliation:** Target node assumes authoritative ownership; source node rolls back unconfirmed migrations.

---

## 5. Verification & Test Coverage

The durability and disaster recovery mechanisms are validated by automated continuous integration suites:
- `tests/durable_commit_semantics.rs`: Tests physical `fdatasync` guarantees, simulated power cutoffs, and truncated write recovery.
- `tests/real_socket_cluster_and_sigkill.rs`: Tests child worker processes abruptly killed via POSIX `SIGKILL` (`kill -9`) over real loopback UDP sockets.
- `tests/zone_crash_reconstruction.rs`: Tests bit-for-bit reconstruction parity between in-memory state and recovered WAL replay.
