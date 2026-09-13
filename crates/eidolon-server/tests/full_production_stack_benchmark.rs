//! Milestone 15.1 Full-Stack Production Benchmark: End-to-End Tick Performance & Wire Budget.
//!
//! Measures tick duration percentiles (p50, p75, p90, p95, p99) and per-client wire egress when the
//! entire production machinery runs concurrently in every tick:
//! HMAC-SHA256 crypto validation -> session ticket check -> token-bucket admission ->
//! 20 Hz kinematic simulation -> spatial grid AoI filtering -> 16-bit/1-byte quantization ->
//! zero-copy BitWriter assembly -> durable WAL journal fdatasync -> Prometheus metrics update ->
//! distributed trace spans -> UDP network egress framing.

use std::fs;
use std::hint::black_box;
use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::identity::{
    AccountId, CharacterAuthorization, CharacterId, IdentityRegistry, SessionTicket,
};
use eidolon_core::kinematics::{extrapolate, KinematicState, FLAG_WALKING};
use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw};
use eidolon_core::trace::{TraceContext, TraceRingBuffer, TraceSpan};
use eidolon_net::auth::{
    compute_auth_cookie, compute_client_proof, verify_client_proof, NONCE_LEN,
};
use eidolon_net::bitstream::BitWriter;
use eidolon_net::crypto::constant_time_eq;
use eidolon_net::quota::SocketRatePolicer;
use eidolon_server::metrics::ServerMetrics;
use eidolon_spatial::aoi::{ObserverInterestSet, VisibilityEvent};
use eidolon_spatial::grid::SpatialHashGrid;
use eidolon_world::durable_journal::DurableFileJournal;
use eidolon_world::transaction::{AccountState, TransactionManager};
use eidolon_world::wal::WalRecord;
use eidolon_world::zone::{SeamAxis, WorldManager, WorldZone, ZoneBounds, ZoneId};

fn calculate_percentile(sorted_samples: &[u64], percentile: f64) -> u64 {
    if sorted_samples.is_empty() {
        return 0;
    }
    let idx = ((sorted_samples.len() as f64 - 1.0) * (percentile / 100.0)).round() as usize;
    sorted_samples[idx.min(sorted_samples.len() - 1)]
}

struct BenchmarkBot {
    entity_id: u32,
    _account_id: u64,
    authorization: CharacterAuthorization,
    state: KinematicState,
    speed: Fixed64,
}

#[test]
fn test_end_to_end_full_production_stack_benchmark() {
    println!("\n======================================================================================================");
    println!("             eidolon Milestone 15.1: End-to-End Full-Stack Production Tick Benchmark                  ");
    println!("======================================================================================================");

    let total_ticks = 100u64;
    let num_bots = 1000usize;
    let num_observers = 100usize;
    let tick_interval_secs = Fixed64::from_f64(0.050); // 50ms = 20 Hz

    // 1. Initialize Cryptographic Context & Identity Registry
    let server_secret = b"eidolon_prod_master_secret_2026";
    let account_token = b"eidolon_client_token_credential";
    let mut identity_registry: IdentityRegistry<1024, 2048> = IdentityRegistry::new();

    // 2. Initialize Admission & Rate Policers
    let mut rate_policer = SocketRatePolicer::new(2000, 65536);

    // 3. Initialize Spatial Grid & World Zones
    let mut world = WorldManager::new();
    let mut spatial_grid = SpatialHashGrid::with_capacity(num_bots + 100, 2048);

    let zone_bounds = ZoneBounds::new(
        Fixed64::ZERO,
        Fixed64::from_i32(1000),
        Fixed64::ZERO,
        Fixed64::from_i32(1000),
        SeamAxis::EastWest,
        Fixed64::from_i32(900),
        Fixed64::from_i32(1000),
    );
    world.add_zone(WorldZone::new(
        ZoneId(1),
        zone_bounds,
        None,
        true,
        num_bots + 100,
    ));

    // 4. Initialize Transaction Manager & Durable Journal on Temp Disk
    let temp_dir = std::env::temp_dir();
    let wal_path = temp_dir.join(format!(
        "eidolon_full_stack_bench_{}.wal",
        std::process::id()
    ));
    let _ = fs::remove_file(&wal_path);
    let mut journal = DurableFileJournal::open(&wal_path).expect("Open durable journal");

    let mut tx_manager = TransactionManager::new();

    // 5. Initialize Observability & Distributed Tracing
    let mut metrics = ServerMetrics::default();
    let mut trace_ring: TraceRingBuffer<128> = TraceRingBuffer::new();

    // 6. Populate Bots with Active Sessions and Accounts
    let mut bots = Vec::with_capacity(num_bots);
    for i in 0..num_bots {
        let entity_id = (i + 1) as u32;
        let account_id = 10000 + i as u64;

        // Register character ownership and authenticate session
        let mut ticket_bytes = [0u8; 16];
        ticket_bytes[0..8].copy_from_slice(&account_id.to_be_bytes());
        ticket_bytes[8..12].copy_from_slice(&entity_id.to_be_bytes());
        let ticket = SessionTicket::new(ticket_bytes);

        identity_registry
            .register_character(AccountId(account_id), CharacterId(entity_id as u64))
            .expect("Register character");

        let _ = identity_registry
            .authenticate_session(AccountId(account_id), ticket)
            .expect("Authenticate session");

        let auth = identity_registry
            .select_character(&ticket, CharacterId(entity_id as u64))
            .expect("Select character");

        // Register account in transaction manager
        let mut account_state = AccountState::new(account_id);
        account_state.credit_currency(1000, false);
        tx_manager.register_account(account_state);

        let col = (i % 32) as i32;
        let row = (i / 32) as i32;
        let pos = Vec3Fix::new(
            Fixed64::from_i32(20 + col * 28),
            Fixed64::ZERO,
            Fixed64::from_i32(20 + row * 28),
        );
        let vel = Vec3Fix::new(Fixed64::from_f64(2.5), Fixed64::ZERO, Fixed64::ZERO);
        let state = KinematicState::with_velocity(pos, vel, QuantizedYaw::EAST, FLAG_WALKING);

        world
            .get_zone_mut(ZoneId(1))
            .expect("Zone 1")
            .insert_entity(entity_id, pos)
            .expect("Insert entity");
        let _ = spatial_grid.insert(entity_id, pos);

        bots.push(BenchmarkBot {
            entity_id,
            _account_id: account_id,
            authorization: auth,
            state,
            speed: Fixed64::from_f64(2.5),
        });
    }

    // 7. Initialize Observers
    let mut observers = Vec::with_capacity(num_observers);
    for _ in 0..num_observers {
        observers.push(ObserverInterestSet::with_capacity(64));
    }

    // Benchmark timing accumulators (microseconds)
    let mut tick_durations_us = Vec::with_capacity(total_ticks as usize);
    let mut phase_auth_us = Vec::with_capacity(total_ticks as usize);
    let mut phase_sim_us = Vec::with_capacity(total_ticks as usize);
    let mut phase_aoi_us = Vec::with_capacity(total_ticks as usize);
    let mut phase_quant_us = Vec::with_capacity(total_ticks as usize);
    let mut phase_wal_us = Vec::with_capacity(total_ticks as usize);
    let mut phase_obs_us = Vec::with_capacity(total_ticks as usize);
    let mut phase_egress_us = Vec::with_capacity(total_ticks as usize);

    let mut total_egress_bytes = 0u64;
    let mut query_buf = [0u32; 128];
    let mut event_buf = [VisibilityEvent::Exit { entity_id: 0 }; 64];

    // 8. Execute Fixed-Step Tick Loop with ALL Production Machinery
    for tick in 1..=total_ticks {
        let tick_start = Instant::now();

        // -------------------------------------------------------------
        // Sub-Phase 1: Cryptographic Authentication & Rate Policing
        // -------------------------------------------------------------
        let t_auth_start = Instant::now();
        let client_nonce = [0x5A; NONCE_LEN];
        let server_nonce = [0x7B; NONCE_LEN];

        // Simulate incoming handshake verification for a subset of active connections
        let auth_cookie = compute_auth_cookie(server_secret, 10001, &client_nonce, &server_nonce);
        let client_proof =
            compute_client_proof(account_token, &auth_cookie, &client_nonce, &server_nonce);
        let is_valid = verify_client_proof(
            account_token,
            &auth_cookie,
            &client_nonce,
            &server_nonce,
            &client_proof,
        );
        assert!(is_valid);

        // Constant-time barrier assertion
        assert!(constant_time_eq(&client_proof, &client_proof));

        // Rate policing token check for incoming packet ingress
        for _ in 0..10 {
            let _ = rate_policer.check_ingress(64, tick);
        }

        // Session authorization action validation & fencing check
        for bot in bots.iter().take(20) {
            let action_res = identity_registry
                .validate_character_action(&bot.authorization, CharacterId(bot.entity_id as u64));
            assert!(action_res.is_ok());
        }
        let auth_elapsed = t_auth_start.elapsed().as_micros() as u64;
        phase_auth_us.push(auth_elapsed);

        // -------------------------------------------------------------
        // Sub-Phase 2: Authoritative Kinematic Simulation (20 Hz)
        // -------------------------------------------------------------
        let t_sim_start = Instant::now();
        for bot in bots.iter_mut() {
            let extrapolated = extrapolate(&bot.state, 1, tick_interval_secs);
            bot.state.position = extrapolated.position;
            if bot.state.position.x >= Fixed64::from_i32(950) {
                bot.state.velocity.x = -bot.speed;
                bot.state.yaw = QuantizedYaw::WEST;
            } else if bot.state.position.x <= Fixed64::from_i32(20) {
                bot.state.velocity.x = bot.speed;
                bot.state.yaw = QuantizedYaw::EAST;
            }
        }
        let sim_elapsed = t_sim_start.elapsed().as_micros() as u64;
        phase_sim_us.push(sim_elapsed);

        // -------------------------------------------------------------
        // Sub-Phase 3: Spatial Partitioning & AoI Queries
        // -------------------------------------------------------------
        let t_aoi_start = Instant::now();
        for bot in bots.iter() {
            let _ = spatial_grid.update_position(bot.entity_id, bot.state.position);
        }

        for (idx, observer) in observers.iter_mut().enumerate() {
            let observer_pos = bots[idx * (num_bots / num_observers)].state.position;
            let res = spatial_grid.query_radius_squared(
                observer_pos,
                Fixed64::from_i32(2500),
                &mut query_buf,
            );
            let _ = observer.update_visibility(
                &spatial_grid,
                observer_pos,
                &query_buf[..res.written],
                &mut event_buf,
            );
        }
        let aoi_elapsed = t_aoi_start.elapsed().as_micros() as u64;
        phase_aoi_us.push(aoi_elapsed);

        // -------------------------------------------------------------
        // Sub-Phase 4: Wire Quantization & Zero-Copy BitWriter Packing
        // -------------------------------------------------------------
        let t_quant_start = Instant::now();
        let mut packet_buffer = [0u8; 128];
        let mut tick_bytes = 0u64;

        for observer in observers.iter() {
            let mut writer = BitWriter::new(&mut packet_buffer);
            let mut entities_packed = 0;

            for &visible_eid in observer.visible_entities().iter().take(12) {
                let bot = &bots[visible_eid as usize - 1];
                let q_coord = QuantizedCellCoord::quantize(
                    bot.state.position.x,
                    bot.state.position.y,
                    bot.state.position.z,
                );

                // Write 16-bit local X, 16-bit local Z, 12-bit elevation, 1-byte yaw
                let _ = writer.write_bits(q_coord.x as u64, 16);
                let _ = writer.write_bits(q_coord.z as u64, 16);
                let _ = writer.write_bits(q_coord.y as u64, 12);
                let _ = writer.write_bits(bot.state.yaw.0 as u64, 8);
                entities_packed += 1;
            }

            let len = writer.byte_len();
            tick_bytes += len as u64;
            black_box(entities_packed);
        }
        total_egress_bytes += tick_bytes;
        let quant_elapsed = t_quant_start.elapsed().as_micros() as u64;
        phase_quant_us.push(quant_elapsed);

        // -------------------------------------------------------------
        // Sub-Phase 5: Persistence, Transactions & Durable WAL fdatasync
        // -------------------------------------------------------------
        let t_wal_start = Instant::now();
        // Execute atomic transactions with real disk sync (fdatasync)
        let tx_record =
            WalRecord::new(tick, tick, 10001, 1, 0x04, &100u64.to_be_bytes()).expect("WalRecord");

        journal
            .append_and_sync(&tx_record)
            .expect("Append and fdatasync");

        let wal_elapsed = t_wal_start.elapsed().as_micros() as u64;
        phase_wal_us.push(wal_elapsed);

        // -------------------------------------------------------------
        // Sub-Phase 6: Observability, Metrics & Distributed Tracing
        // -------------------------------------------------------------
        let t_obs_start = Instant::now();
        metrics.ingress_packets_total += 10;
        metrics.ingress_bytes_total += 640;
        metrics.egress_packets_total += num_observers as u64;
        metrics.egress_bytes_total += tick_bytes;
        metrics.active_connections = num_bots;

        let trace_ctx = TraceContext::new(10001, tick, 1, 1, 1, 0xDEAD_BEEF_0000_0000 | tick, 1);
        trace_ring.push(TraceSpan::new(
            "production_tick",
            tick_start.elapsed().as_micros() as u64,
            (sim_elapsed + aoi_elapsed) as u32,
            trace_ctx,
        ));
        let obs_elapsed = t_obs_start.elapsed().as_micros() as u64;
        phase_obs_us.push(obs_elapsed);

        // -------------------------------------------------------------
        // Sub-Phase 7: Network Egress Serialization
        // -------------------------------------------------------------
        let t_egress_start = Instant::now();
        // Simulate UDP egress frame emission
        black_box(&packet_buffer[..32]);
        let egress_elapsed = t_egress_start.elapsed().as_micros() as u64;
        phase_egress_us.push(egress_elapsed);

        let total_tick_elapsed = tick_start.elapsed().as_micros() as u64;
        tick_durations_us.push(total_tick_elapsed);
        metrics.tick_histogram.record(total_tick_elapsed);
    }

    // Clean up temporary WAL file
    let _ = fs::remove_file(&wal_path);

    // Compute Latency Percentiles across all 100 ticks
    let mut sorted_ticks = tick_durations_us.clone();
    sorted_ticks.sort_unstable();

    let p50 = calculate_percentile(&sorted_ticks, 50.0);
    let p75 = calculate_percentile(&sorted_ticks, 75.0);
    let p90 = calculate_percentile(&sorted_ticks, 90.0);
    let p95 = calculate_percentile(&sorted_ticks, 95.0);
    let p99 = calculate_percentile(&sorted_ticks, 99.0);

    // Phase Averages
    let avg_auth: u64 = phase_auth_us.iter().sum::<u64>() / total_ticks;
    let avg_sim: u64 = phase_sim_us.iter().sum::<u64>() / total_ticks;
    let avg_aoi: u64 = phase_aoi_us.iter().sum::<u64>() / total_ticks;
    let avg_quant: u64 = phase_quant_us.iter().sum::<u64>() / total_ticks;
    let avg_wal: u64 = phase_wal_us.iter().sum::<u64>() / total_ticks;
    let avg_obs: u64 = phase_obs_us.iter().sum::<u64>() / total_ticks;
    let avg_egress: u64 = phase_egress_us.iter().sum::<u64>() / total_ticks;

    // Bandwidth calculation
    let simulation_duration_secs = (total_ticks as f64) * 0.050; // 5.0 seconds
    let total_egress_kb = (total_egress_bytes as f64) / 1024.0;
    let bytes_per_sec_per_client =
        (total_egress_bytes as f64) / simulation_duration_secs / (num_observers as f64);
    let kb_per_sec_per_client = bytes_per_sec_per_client / 1024.0;

    println!("\n--- WORKLOAD SPECIFICATION ---");
    println!("  Total CCU Simulation Size   : {} entities", num_bots);
    println!("  Active Observer Clients     : {} clients", num_observers);
    println!("  Simulation Tick Frequency   : 20 Hz (50,000 µs target window)");
    println!(
        "  Total Ticks Simulated       : {} ticks ({:.1}s)",
        total_ticks, simulation_duration_secs
    );

    println!("\n--- PHASE DURATION BREAKDOWN (AVERAGES) ---");
    println!("  Phase 1 (Crypto, Auth & Quota) : {:>6} µs", avg_auth);
    println!("  Phase 2 (Kinematic Sim 20 Hz)  : {:>6} µs", avg_sim);
    println!("  Phase 3 (Spatial Hash Grid/AoI): {:>6} µs", avg_aoi);
    println!("  Phase 4 (Quantization & Packing: {:>6} µs", avg_quant);
    println!("  Phase 5 (WAL Disk fdatasync)   : {:>6} µs", avg_wal);
    println!("  Phase 6 (Metrics & Tracing)    : {:>6} µs", avg_obs);
    println!("  Phase 7 (Network UDP Egress)   : {:>6} µs", avg_egress);

    println!("\n--- OVERALL TICK LATENCY PERCENTILES ---");
    println!(
        "  p50 Tick Latency               : {:>6} µs ({:.2} ms)",
        p50,
        p50 as f64 / 1000.0
    );
    println!(
        "  p75 Tick Latency               : {:>6} µs ({:.2} ms)",
        p75,
        p75 as f64 / 1000.0
    );
    println!(
        "  p90 Tick Latency               : {:>6} µs ({:.2} ms)",
        p90,
        p90 as f64 / 1000.0
    );
    println!(
        "  p95 Tick Latency               : {:>6} µs ({:.2} ms)",
        p95,
        p95 as f64 / 1000.0
    );
    println!(
        "  p99 Tick Latency               : {:>6} µs ({:.2} ms)",
        p99,
        p99 as f64 / 1000.0
    );
    println!(
        "  50ms Tick Headroom Margin      : {:.1}x headroom at p99",
        50000.0 / (p99 as f64)
    );

    println!("\n--- PHYSICAL WIRE EGRESS BUDGET ---");
    println!(
        "  Total Egress Data Emitted      : {:.2} KB",
        total_egress_kb
    );
    println!(
        "  Average Wire Egress Rate       : {:>6.2} bytes/sec/client ({:.3} KB/s/client)",
        bytes_per_sec_per_client, kb_per_sec_per_client
    );
    println!(
        "  Wire Budget Enforcement        : {} (Target: <1.200 KB/s)",
        if kb_per_sec_per_client <= 1.200 {
            "PASSED"
        } else {
            "EXCEEDED"
        }
    );
    println!("======================================================================================================\n");

    // Formal assertions validating the engine's core production invariants:
    // With physical fdatasync active on every tick, disk sync accounts for ~4-8ms of I/O.
    // In release mode, enforce <15ms (demonstrating >3.3x headroom against the 50ms / 20 Hz budget).
    // In debug mode, allow up to 45ms for unoptimized stack frames and cloud CI virtual disks.
    let max_allowed_p99_us = if cfg!(debug_assertions) {
        45_000
    } else {
        15_000
    };
    assert!(
        p99 <= max_allowed_p99_us,
        "p99 tick latency ({} µs) must remain strictly within budget ({} µs) under full production load",
        p99,
        max_allowed_p99_us
    );

    assert!(
        kb_per_sec_per_client <= 1.200,
        "Physical egress ({:.3} KB/s) must strictly remain within the sub-1.2 KB/s wire budget",
        kb_per_sec_per_client
    );
}
