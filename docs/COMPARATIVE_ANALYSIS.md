# Architectural Comparison: Eidolon, Nakama, Colyseus, and SpacetimeDB

## 1. Executive Summary and Taxonomy

In the modern multiplayer games industry, developers face distinct categories of networking and backend challenges. Choosing a server technology is not a simple checklist comparison: different engines are engineered to solve fundamentally different low-level problems.

```text
┌────────────────────────────────────────────────────────────────────────────────────────────────┐
│                               MULTIPLAYER ARCHITECTURAL TAXONOMY                               │
├───────────────────────┬────────────────────────────────────────────────────────────────────────┤
│ Meta-Backend Platform │ Nakama (Heroic Labs): Accounts, social graph, leaderboards, purchases,  │
│                       │ chat, tournaments, storage, and cross-platform identity.               │
├───────────────────────┼────────────────────────────────────────────────────────────────────────┤
│ Room-Based Framework  │ Colyseus: Authoritative match rooms, state synchronization, lobbies,   │
│                       │ and rapid integration for session-based multiplayer (4-64 players).    │
├───────────────────────┼────────────────────────────────────────────────────────────────────────┤
│ Server-Database Engine│ SpacetimeDB (Clockwork Labs): Relational database-as-server model,     │
│                       │ client-side SQL subscriptions, and integrated in-memory transactions.  │
├───────────────────────┼────────────────────────────────────────────────────────────────────────┤
│ Spatial Sim & Wire    │ Eidolon: High-frequency (20 Hz) authoritative spatial simulation,       │
│ Kernel                │ sub-1.2 KB/s wire quantization, tiered AoI, and scale-to-zero EoS      │
│                       │ economics for massive persistent worlds (1,000-50,000+ players).       │
└───────────────────────┴────────────────────────────────────────────────────────────────────────┘
```

`eidolon` does not seek to replace general-purpose backend platforms like Nakama or match room frameworks like Colyseus. Instead, `eidolon` solves the narrow, exceptionally difficult core of multiplayer games: **how to simulate and synchronize massive shared-world spatial densities at high frequency without incurring bankruptcy-level cloud network egress bills.**

---

## 2. In-Depth Comparative Matrix

| Architectural Dimension | Eidolon | Nakama (Heroic Labs) | Colyseus | SpacetimeDB (Clockwork Labs) |
| :--- | :--- | :--- | :--- | :--- |
| **Primary Domain** | Authoritative MMO Spatial & Wire Kernel | Enterprise Out-of-Band Game Backend | Room-Based Authoritative Multiplayer | Relational Database & Server Hybrid |
| **Target Scale** | 5,000,000 CCU cluster; 50,000 in continuous world | 2,000,000+ CCU meta-backend | 100,000+ CCU (room sharding) | Scalable relational world state |
| **Spatial Density in Shared Region** | **Exceptional:** 1,000+ entities in immediate AoI via tiered filtering | **Moderate:** Session rooms or custom Go/Lua spatial hooks | **Room-Bounded:** Typically 8 to 64 players per room | **Relational:** Bounded by in-memory table scan and SQL queries |
| **Network Wire Budget** | **<1.2 KB/s per client** (Quantized 16-bit, 1-byte yaw, dead reckoning) | Standard binary / Protobuf / JSON (~10 to 30 KB/s) | Schema-based delta compression (~5 to 20 KB/s) | Relational row updates / binary BSATN (~5 to 25 KB/s) |
| **Simulation Loop** | Fixed 20 Hz deterministic tick loop; **zero dynamic allocations** | Event-driven or custom tick in Go / TypeScript / Lua | Fixed tick loop in TypeScript / Node.js | Event-driven reducers invoked on transactional commits |
| **Language & Runtime** | Pure native Rust (`core` / `std` only); zero third-party dependencies | Go runtime with CockroachDB / PostgreSQL | TypeScript / JavaScript on Node.js / Bun | Rust / C# / WebAssembly with in-memory relational engine |
| **Area of Interest (AoI)** | 3-tier spatial frequency grid (Immediate 10 Hz, Mid 2 Hz, Horizon events) | Application-level room broadcast or custom hooks | Room state delta broadcast | Spatial index queries over relational tables |
| **Persistence & Durability** | Append-only WAL with `fdatasync` and distributed quorum (RPO = 0) | CockroachDB / PostgreSQL relational persistence | Redis / MongoDB / external database drivers | In-memory relational tables with durable transaction log |
| **Long-Tail EoS Hosting** | **Scale-to-zero** on Google Cloud free tier ($0/month for 5 players) | Requires minimum database and backend cluster ($50-300+/month) | Small VPS instance ($10-20/month) | Cloud cluster or dedicated server instance |
| **Out-of-Band Meta Services** | None (delegated to companion backends like Nakama) | **Comprehensive:** Leaderboards, chat, guilds, IAP, matchmaking | Matchmaking and lobby rooms | Relational queries and stored procedures |

---

## 3. Detailed Platform Profiles

### 3.1 Nakama (Heroic Labs)
**The Industry-Standard Enterprise Meta-Backend:**
- **Strengths:** Nakama is an extraordinarily mature, battle-tested open-source backend powering major commercial titles with millions of concurrent users. It provides out-of-the-box implementations of social graphs, user accounts, authentication providers (Apple, Google, Steam, Epic), chat channels, real-time matchmaking, competitive tournaments, virtual wallets, and storage collections.
- **Architectural Trade-Off:** Nakama is designed as a broad, enterprise-grade game backend. While it supports authoritative multiplayer matches, its networking is general-purpose. It does not provide specialized MMO sub-millimeter coordinate quantization, asymmetric vertical elevation bitpacking, or tiered spatial frequency hashing.
- **Verdict:** If you need to build user accounts, matchmaking, clans, leaderboards, and store inventory for a game shipping soon, Nakama is the undisputed leader.

### 3.2 Colyseus
**The Developer-Friendly Authoritative Room Framework:**
- **Strengths:** Colyseus is built specifically for room-based authoritative multiplayer games. Its TypeScript schema model provides automatic state synchronization, client-side prediction, interpolation, and seamless integration with engines like Unity, Unreal, Godot, and web frontends.
- **Architectural Trade-Off:** Colyseus is optimized around the room abstraction. It excels when players are segmented into distinct matches (e.g. 4-player co-op, 10-player battle arenas, 32-player party games). It is not designed to coordinate a continuous, seamless open world where 50,000 players navigate across zone boundaries without loading screens.
- **Verdict:** If your game is structured into discrete lobbies, matches, or battle instances, Colyseus provides an exceptionally rapid, clean development workflow.

### 3.3 SpacetimeDB (Clockwork Labs)
**The Database-Is-The-Server Paradigm:**
- **Strengths:** SpacetimeDB eliminates the traditional architectural division between the game server and the database. By treating the database as the game server, clients subscribe directly to SQL-like relational queries and execute transactions through WebAssembly reducers. This provides unified state management and transactional consistency.
- **Architectural Trade-Off:** While intellectually elegant, representing high-frequency kinematics (20-60 Hz continuous physics displacement) as relational table mutations introduces computational overhead compared to data-oriented SIMD arrays and bitstream quantization.
- **Verdict:** An innovative and compelling approach for persistent shared-world games with rich relational state and complex emergent rules.

### 3.4 Eidolon
**The Low-Level Spatial Simulation and Wire Kernel:**
- **Strengths:** Eidolon attacks the problem from the opposite direction: hardware mechanical sympathy, bit-level wire optimization, and deterministic spatial execution. By engineering an asymmetric quantization scheme (16-bit horizontal, 12-bit vertical, 1-byte yaw) paired with intent-based dead reckoning and tiered spatial frequency hashing, Eidolon reduces client egress to 0.5-1.2 KB/s. This transforms the economics of hosting massive persistent shared worlds, enabling 50,000+ concurrent players in continuous space.
- **Architectural Trade-Off:** Eidolon is strictly a simulation and networking kernel. It intentionally does not provide social graphs, global chat, leaderboards, or web admin dashboards.
- **Verdict:** The premier open-source foundation for projects where spatial scale, continuous density, and egress bandwidth economics are the primary architectural constraints.

---

## 4. The "Better at What?" Engineering Decision Framework

When evaluating which engine to adopt for a project:

```text
                                 What is your primary architectural challenge?
                                                      │
              ┌───────────────────────────────────────┼───────────────────────────────────────┐
              ▼                                       ▼                                       ▼
    "I need a complete                      "I need room-based                     "I need an open world
     game backend with                       multiplayer matches                    with massive density
     social, accounts,                       with rapid client                      and low bandwidth
     and leaderboards."                      state sync."                           costs."
              │                                       │                                       │
              ▼                                       ▼                                       ▼
        Choose NAKAMA                           Choose COLYSEUS                         Choose EIDOLON
```

1. **Choose Nakama when:**
   - Your game requires user authentication, social graphs, party matchmaking, global leaderboards, tournaments, and real-money store purchases.
   - You want a battle-tested platform supporting millions of concurrent accounts out of the box.
2. **Choose Colyseus when:**
   - Your gameplay is room-centric (battle royales, card games, party brawlers, dungeon instances).
   - Your team writes TypeScript or wants automatic schema-driven client synchronization.
3. **Choose SpacetimeDB when:**
   - You want to eliminate the ORM and database-server boundary entirely.
   - Your game mechanics are heavily relational and benefit from client SQL subscriptions.
4. **Choose Eidolon when:**
   - You are building an open-world MMO, virtual world, or high-density shared environment.
   - Network egress costs at scale represent a commercial threat ($20,000-$200,000+/month bandwidth bills).
   - You require deterministic, zero-allocation 20 Hz tick simulation in native Rust.
   - You want scale-to-zero economics so the game can run for $0-$5/month indefinitely during long-tail maintenance.

---

## 5. The Symbiotic Architecture: Eidolon + Nakama

The most powerful production MMO architecture does not treat Eidolon and Nakama as competitors: it deploys them together as complementary layers of the stack.

```text
                                 COMMERCIAL MMO PRODUCTION TOPOLOGY
                                 
  [ Client Application ]
       │            │
       │ (Egress < 1.2 KB/s UDP)
       │            │
       ▼            │ (HTTPS / WebSockets RPC)
  ┌───────────────────────────────┐         ▼
  │         EIDOLON CLUSTER       │   ┌───────────────────────────────┐
  │  (Authoritative World Engine) │   │        NAKAMA CLUSTER         │
  ├───────────────────────────────┤   │      (Meta-Game Platform)     │
  │ • Spatial Hash Grid & AoI     │   ├───────────────────────────────┤
  │ • Deterministic 20 Hz Ticks   │   │ • Account Authentication      │
  │ • Quantized Wire Protocol     │   │ • Friends & Guild Social Graph│
  │ • Dead Reckoning Extrapolation│   │ • Global Chat Channels        │
  │ • Seamless Zone Handoffs      │   │ • Matchmaking & Tournaments   │
  │ • Scale-to-Zero Co-op Rooms   │   │ • In-App Purchases & Wallet   │
  └──────────────┬────────────────┘   └──────────────┬────────────────┘
                 │                                   │
                 ▼                                   ▼
          [ Durable WAL ]                    [ CockroachDB ]
        (fdatasync / Quorum)             (Relational Meta State)
```

In this hybrid topology:
1. **Nakama** handles initial player authentication, friend lists, guild management, text chat, competitive leaderboards, and store transactions out-of-band.
2. Once the player enters the 3D world, Nakama issues an authenticated session ticket handing the connection over to the **Eidolon** cluster.
3. **Eidolon** hosts the real-time, high-frequency spatial simulation, broadcasting entity transforms within the sub-1.2 KB/s wire budget and enforcing authoritative boundary handoffs.
4. Upon major economic milestones or disconnects, Eidolon checkpoints the player state back to cold storage or Nakama storage.

This architecture leverages the definitive strengths of both platforms: Nakama's enterprise meta-backend tooling combined with Eidolon's ultra-low-bandwidth spatial simulation kernel.
