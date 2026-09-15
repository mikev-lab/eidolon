# Persistence & Database Integration Runbook: Storing Accounts, Inventories, Buildings & Economies

**Classification:** Developer Integration & Production Architecture Guide  
**Target Audience:** Backend Engineers, Technical Directors, and Database Administrators  

---

## 1. The Core Invariant: Memory-Authoritative Simulation

In a high-concurrency multiplayer game server running at 20 Hz (50ms tick intervals), **synchronous database queries inside the game tick loop are strictly prohibited**.

If a player crafts an item, loots a chest, or places a building, executing a synchronous SQL query (`UPDATE player_inventories SET ...`) introduces 5ms to 50ms of network latency, connection pool lock contention, and disk write stalls. Under high concurrent load, this causes cascading tick overruns and catastrophic server lag.

`eidolon` solves this through a **Memory-Authoritative Runtime paired with Tiered Asynchronous Persistence**:

```text
[ Client Action (UDP) ]
          │  (e.g., Craft Item / Build Wall)
          ▼
[ Authoritative Server Tick ] ──────> Mutates RAM State (Microseconds, Zero GC)
          │
          ├──────────────────────────────────────────────────────┐
          ▼                                                      ▼
   [ In-Memory WAL ]                                      [ Client ACK ]
     - WalRingBuffer (wal.rs)                               (Dispatched immediately)
     - DurableFileJournal (POSIX fdatasync, RPO = 0)
          │
          ▼ (Asynchronous Batch Drain)
[ Background Persistence Sink (DurableWalSink) ]
          │
          ├───> [ Relational Tables ] (PostgreSQL / MySQL)
          │       - Normalized items, currencies, trade ledger, web portals
          │
          └───> [ Cold Snapshot Store ] (PostgreSQL BYTEA / S3 / Blob Store)
                  - <1 KB compact binary blobs for sub-5ms hydration
```

1. **In-Memory Authoritative State:** Memory is the primary source of truth during gameplay. Inventories, character stats, and structure chunks mutate in RAM with zero heap allocations.
2. **Write-Ahead Logging (WAL):** Every mutation appends a binary record to an in-memory ring buffer. For high-value transactions (gacha pulls, trades, currency debits), disk durability is secured via `fdatasync` before sending the acknowledgment packet to the client, guaranteeing a **Recovery Point Objective of zero (RPO = 0)**.
3. **Decoupled Background Persistence:** A background thread drains WAL batches asynchronously and persists them into your external database (PostgreSQL, MySQL, SQLite) without touching the hot simulation loop.
4. **Binary Hibernation Snapshots:** Inactive players and dormant zones are serialized into compact binary snapshots (<1 KB), enabling instances to scale to zero for **$0 to $5/month hosting costs**.

---

## 2. Database Selection Guide & Architecture Comparison

Choosing the right storage technology determines operational costs, maintenance overhead, and scalability:

| Database Technology | Recommendation Level | Typical Monthly Cost | Primary Strength | Architectural Verdict |
| :--- | :--- | :--- | :--- | :--- |
| **PostgreSQL** | **Strongly Recommended** | $0 to $15 / month | Full ACID, JSONB, native `BYTEA` binary blobs, robust connection pooling | **Best overall choice** for production MMO backends, economies, and player bases. |
| **SQLite / Local NVMe** | **Strongly Recommended (Indie/Single Node)** | $0 / month (Local Disk) | Zero network hops, zero cloud egress, in-process execution | **Ideal for single-server launches**, test environments, and ultra-low-budget indie hosting. |
| **MySQL / MariaDB** | **Viable Alternative** | $5 to $20 / month | Widespread operational familiarity, high-throughput replication | **Viable** if your engineering team already has dedicated MySQL infrastructure. |
| **ScyllaDB / DynamoDB** | **Recommended for High Scale (1M+ CCU)** | Usage-Based Cloud Pricing | Global multi-region key-value partitioning | **Excellent for global account hibernation blobs**; pair with Postgres for relational trade ledgers. |
| **Firebase / Firestore** | **Not Recommended for Core Game Loop** | Unbounded (Per-Read/Write Billing) | Easy out-of-band mobile authentication | **Avoid for active gameplay.** Billing per document read/write will cause extreme costs at 20 Hz. |

### Why PostgreSQL is the Preferred Choice for eidolon
1. **Binary Storage (`BYTEA`):** Directly stores `eidolon`'s `<1 KB` binary hibernation profiles and 256m building chunk manifests with zero encoding overhead.
2. **ACID Relational Guarantees:** Ensures item transfers, auction house trades, and currency spends are mathematically immune to duplicate-spend exploits.
3. **JSONB Affixes:** Allows storing unique procedural weapon stats, sockets, and enchantments without rigid schema migrations.
4. **Scale-to-Zero Compatibility:** Compatible with serverless Postgres providers (e.g. Neon, Supabase) or single-container instances on Google Cloud Free Tier (`e2-micro`), maintaining our sub-$5/month operational target.

---

## 3. Network Egress and Cloud Hosting Invariants

A critical design concern for MMO developers is whether connecting to an external database will degrade network bandwidth or inflate cloud egress bills.

### Invariant 1: Zero Impact on Public Client Wire Budget (<1.2 KB/s)
The engine's sub-1KB/s wire budget governs traffic over the public internet between game clients and the server via UDP. Database communication occurs entirely over internal backend networks. **Zero bytes of database traffic are ever transmitted to game clients.**

### Invariant 2: Internal Cloud Network Traffic is 100% Free
On all major cloud providers (AWS, GCP, Azure, DigitalOcean):
- **Public Internet Egress:** Billed between $0.05 and $0.12 per GB.
- **Internal VPC Traffic:** Billed at **$0.00 per GB (Free)** within the same Virtual Private Cloud, availability zone, or local Kubernetes cluster.
- Co-locating your PostgreSQL instance in the same private subnet or Kubernetes cluster as `eidolon-server` incurs **$0.00 in cloud egress fees**.

### Invariant 3: 95% Reduction in Database I/O via Flat Binary Packing
Traditional MMO backends serialize player states to verbose JSON documents (15 KB to 50 KB per player save). `eidolon` uses flat binary serialization:
- Account hibernation snapshots: **200 to 800 bytes** (<1 KB).
- Building mutations: **20 bytes** (`StructureDeltaPacket`).
- WAL records: **30-byte header** with inlined binary payload.
This reduces database write volume and backup bandwidth by over 95%.

---

## 4. Production PostgreSQL Database Schema

The following production DDL schema provides the complete relational and snapshot foundation for an `eidolon`-powered MMO:

```sql
-- ============================================================================
-- 1. MASTER PLAYER PROFILES & COLD HIBERNATION SNAPSHOTS
-- Stores identity, login metadata, and compact <1 KB binary snapshot for sub-5ms hydration.
-- ============================================================================
CREATE TABLE player_profiles (
    account_id BIGINT PRIMARY KEY,
    username VARCHAR(32) NOT NULL UNIQUE,
    player_level INT NOT NULL DEFAULT 1,
    free_currency BIGINT NOT NULL DEFAULT 0,
    premium_currency BIGINT NOT NULL DEFAULT 0,
    last_login_epoch BIGINT NOT NULL,
    last_zone_id INT NOT NULL DEFAULT 1,
    last_pos_x DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    last_pos_y DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    last_pos_z DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    
    -- Compact <1 KB binary snapshot produced by PlayerProfile::serialize_snapshot()
    -- Contains pity counters, character roster, and active progression
    hibernation_blob BYTEA NOT NULL,
    snapshot_version SMALLINT NOT NULL DEFAULT 1,
    
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_player_profiles_last_login ON player_profiles (last_login_epoch DESC);

-- ============================================================================
-- 2. NORMALIZED INVENTORY & EQUIPMENT ITEMS
-- Used for web portals, customer support GM tooling, auction houses, and analytics.
-- ============================================================================
CREATE TABLE player_inventory_items (
    item_instance_id BIGSERIAL PRIMARY KEY,
    account_id BIGINT NOT NULL REFERENCES player_profiles(account_id) ON DELETE CASCADE,
    container_type SMALLINT NOT NULL, -- 0 = MainBag, 1 = Equipment, 2 = BankVault
    container_slot SMALLINT NOT NULL, -- 0 to 63
    item_id INT NOT NULL,
    quantity INT NOT NULL DEFAULT 1,
    durability SMALLINT NOT NULL DEFAULT 100,
    flags INT NOT NULL DEFAULT 0,     -- Bit 0: Soulbound, Bit 1: QuestItem, Bit 2: Locked
    
    -- Optional procedural stats (random affixes, sockets, crafter name)
    custom_stats JSONB,
    
    UNIQUE (account_id, container_type, container_slot)
);

CREATE INDEX idx_player_inventory_account ON player_inventory_items (account_id);
CREATE INDEX idx_player_inventory_item_id ON player_inventory_items (item_id);

-- ============================================================================
-- 3. WORLD STRUCTURES & PLAYER BASES (256m x 256m SPATIAL CHUNKS)
-- Maps directly to eidolon_world::chunk_manifest::ChunkCoord
-- ============================================================================
CREATE TABLE world_structure_chunks (
    zone_id INT NOT NULL,
    chunk_x SMALLINT NOT NULL,
    chunk_z SMALLINT NOT NULL,
    manifest_hash INT NOT NULL,       -- Adler-32 hash matching client disk cache
    piece_count INT NOT NULL DEFAULT 0,
    
    -- Compressed binary array of StructureDeltaPacket records (20 bytes per piece)
    chunk_geometry_blob BYTEA NOT NULL,
    last_mutated_tick BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    
    PRIMARY KEY (zone_id, chunk_x, chunk_z)
);

-- ============================================================================
-- 4. IMMUTABLE TRANSACTION AUDIT LEDGER
-- Append-only log for anti-duplication, economic monitoring, and trade disputes.
-- ============================================================================
CREATE TABLE currency_and_trade_ledger (
    transaction_id BIGSERIAL PRIMARY KEY,
    tx_uuid UUID NOT NULL UNIQUE,
    account_id BIGINT NOT NULL,
    tx_type VARCHAR(24) NOT NULL,     -- 'LOOT', 'TRADE', 'GACHA_PULL', 'CRAFT', 'MARKET'
    delta_amount BIGINT NOT NULL,
    balance_after BIGINT NOT NULL,
    target_account_id BIGINT,
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_ledger_account ON currency_and_trade_ledger (account_id, created_at DESC);
CREATE INDEX idx_ledger_tx_uuid ON currency_and_trade_ledger (tx_uuid);
```

---

## 5. End-to-End Implementation Runbook

### Step 1: Sub-Millisecond Player Hydration on Login
When a client connects and completes the initial cryptographic handshake:
1. Query `player_profiles` for the account record.
2. Hydrate the in-memory entity state using `PlayerProfile::deserialize_snapshot`:

```rust
// Example: Server-side account hydration (crates/eidolon-world/src/hibernation.rs)
pub fn hydrate_player_on_connect(
    account_id: u64,
    db_row_blob: &[u8],
) -> Result<PlayerProfile, WorldError> {
    // Executes in sub-millisecond time with zero dynamic heap allocations.
    // Verifies protocol magic (0x45494842) and Adler-32 payload checksum.
    let profile = PlayerProfile::deserialize_snapshot(db_row_blob)?;
    Ok(profile)
}
```

3. Insert the entity into the local spatial grid and attach currency balances to the in-memory transaction coordinator.

---

### Step 2: Logging In-Flight Mutations to the WAL Ring Buffer
During gameplay, all economic mutations append a binary record to the non-blocking ring buffer:

```rust
// Example: Recording an inventory transaction inside the simulation loop
pub fn on_player_loot_item(
    wal: &mut WriteAheadJournal,
    current_tick: u64,
    account_id: u64,
    entity_id: u32,
    item_id: u32,
    quantity: u16,
    slot_idx: u8,
) -> Result<(), WorldError> {
    let mut payload = [0u8; 128];
    payload[0..4].copy_from_slice(&item_id.to_be_bytes());
    payload[4..6].copy_from_slice(&quantity.to_be_bytes());
    payload[6] = slot_idx;

    // Zero-allocation append to pre-allocated ring buffer
    wal.append(
        current_tick,
        account_id,
        entity_id,
        OP_INVENTORY_MUTATION,
        &payload[..7],
    )?;

    Ok(())
}
```

---

### Step 3: Implementing a Custom Asynchronous Database Sink
Connect the `eidolon` persistence pipeline to your database by implementing the native `DurableWalSink` trait:

```rust
// Example: Asynchronous batch flusher implementing DurableWalSink (wal.rs)
use eidolon_world::wal::{DurableWalSink, WalRecord, OP_INVENTORY_MUTATION, OP_CURRENCY_DELTA};
use eidolon_world::error::WorldError;

pub struct PostgresWalSink {
    // Internal batch queue or channel to background async runtime (Tokio / Thread pool)
    sender: std::sync::mpsc::SyncSender<Vec<WalRecord>>,
}

impl DurableWalSink for PostgresWalSink {
    fn write_batch(&mut self, records: &[WalRecord]) -> Result<(), WorldError> {
        // Hand off batch to background worker thread without blocking the simulation tick
        let batch_vec = records.to_vec();
        self.sender.try_send(batch_vec).map_err(|_| {
            WorldError::WalStorageUnavailable
        })?;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), WorldError> {
        // Execute fsync on disk or await database flush confirmation
        Ok(())
    }
}
```

---

### Step 4: Player Disconnect & Cold-State Account Hibernation
When a player disconnects, departs a dungeon instance, or when the server prepares to scale to zero:
1. Serialize the player's active state into a stack-allocated buffer:

```rust
pub fn hibernate_player_on_disconnect(
    profile: &PlayerProfile,
    epoch_now_sec: u64,
) -> Result<[u8; 1024], WorldError> {
    let mut buffer = [0u8; 1024];
    let written_len = profile.serialize_snapshot(epoch_now_sec, &mut buffer)?;
    
    // Pass buffer[..written_len] to background SQL worker:
    // UPDATE player_profiles SET hibernation_blob = $1, updated_at = NOW() WHERE account_id = $2;
    Ok(buffer)
}
```

2. Once all players disconnect from an ephemeral dungeon room or zone shard, the process scales down, consuming **zero active memory and zero cloud compute costs**.

---

## 6. Checklist: Best Practices for Game Production

1. **Keep the 20 Hz Simulation Loop Memory-Only:** Never invoke `sqlx`, `diesel`, or synchronous socket drivers from the tick thread.
2. **Dual-Tier Storage Strategy:** Store compact binary snapshots (`hibernation_blob`) for lightning-fast logins, and update normalized tables (`player_inventory_items`) asynchronously for web dashboards and queries.
3. **Partition Player Buildings by 256m Chunks:** Store structures using `ChunkCoord` and 20-byte `StructureDeltaPacket` slices rather than storing millions of individual piece rows.
4. **Co-Locate Database in Same VPC:** Ensure your database resides in the same cloud region and private network as your game server instances to guarantee **$0.00 network egress costs**.
5. **Enforce RPO = 0 for Trades:** Use `DurableFileJournal` with `fdatasync` before acknowledging high-value currency or gacha item transactions to protect against server crashes.
