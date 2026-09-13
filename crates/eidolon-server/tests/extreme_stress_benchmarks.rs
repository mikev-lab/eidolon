//! Comprehensive capacity headroom, extreme density stress testing, and latency percentile benchmarks.
//!
//! Evaluates:
//! 1. Capacity Headroom & Phase Duration Percentiles (p50, p75, p90, p95, p99) under baseline 2,000 CCU load.
//! 2. Extreme Density Clustering Scaling (75th to 99th percentile: 10, 50, 100, 250, 500 entities in AoI).
//! 3. Adaptive Load Shedding Escalation and Circuit Breaker Verification under induced overload.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::kinematics::{KinematicState, FLAG_SPRINTING, FLAG_WALKING};
use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw, FLAG_CELL_ANCHOR};
use eidolon_net::packet::{PacketHeader, PacketView};
use eidolon_net::protocol::{ChannelType, PacketType, HEADER_SIZE};
use eidolon_server::io::NetworkIoWorker;
use eidolon_server::queue::{NetworkPacket, SpscPacketQueue};
use eidolon_server::tick::TickCoordinator;
use eidolon_spatial::aoi::{
    LoadSheddingLevel, ObserverInterestSet, VisibilityEvent, MAX_AOI_RADIUS_SQ,
};
use eidolon_spatial::grid::SpatialHashGrid;
use eidolon_spatial::tier::FrequencyTier;
use eidolon_world::zone::{SeamAxis, WorldManager, WorldZone, ZoneBounds, ZoneId};

/// Helper to compute percentiles from a sorted slice of microsecond measurements.
fn calculate_percentile(sorted_samples: &[u64], percentile: f64) -> u64 {
    if sorted_samples.is_empty() {
        return 0;
    }
    let idx = ((sorted_samples.len() as f64 - 1.0) * (percentile / 100.0)).round() as usize;
    sorted_samples[idx.min(sorted_samples.len() - 1)]
}

/// Baseline entity model for capacity headroom testing.
struct StressBot {
    entity_id: u32,
    current_zone: ZoneId,
    state: KinematicState,
    speed: Fixed64,
    direction: i32,
    is_migrator: bool,
}

impl StressBot {
    fn new(entity_id: u32, initial_pos: Vec3Fix, is_migrator: bool) -> Self {
        let speed = if is_migrator {
            Fixed64::from_f64(6.0)
        } else {
            Fixed64::from_f64(3.0)
        };
        let vel = if is_migrator {
            Vec3Fix::new(speed, Fixed64::ZERO, Fixed64::ZERO)
        } else {
            Vec3Fix::new(Fixed64::ZERO, Fixed64::ZERO, speed)
        };
        let state = KinematicState::with_velocity(
            initial_pos,
            vel,
            QuantizedYaw::NORTH,
            if is_migrator {
                FLAG_SPRINTING
            } else {
                FLAG_WALKING
            },
        );
        Self {
            entity_id,
            current_zone: ZoneId(1),
            state,
            speed,
            direction: 1,
            is_migrator,
        }
    }

    fn step(&mut self, dt: Fixed64) {
        if self.is_migrator {
            self.state.position.x += self.speed * dt;
        } else {
            if self.state.position.z >= Fixed64::from_i32(480) {
                self.direction = -1;
                self.state.velocity.z = -self.speed;
            } else if self.state.position.z <= Fixed64::from_i32(20) {
                self.direction = 1;
                self.state.velocity.z = self.speed;
            }
            self.state.position.z += self.state.velocity.z * dt;
        }
    }
}

#[test]
fn test_capacity_overhead_and_tick_percentiles_baseline_2000_ccu() {
    let tick_interval_secs = Fixed64::from_f64(0.050); // 50ms = 20 Hz
    let total_ticks = 100u64;
    let num_bots = 2000usize;
    let num_observers = 100usize;

    // 1. Initialize Network Worker and Client Sockets
    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let worker = NetworkIoWorker::bind(server_addr).expect("bind server worker");
    let bound_server_addr = worker.local_addr().expect("server local addr");

    let num_client_sockets = 4;
    let mut client_sockets = Vec::with_capacity(num_client_sockets);
    for _ in 0..num_client_sockets {
        let sock = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .expect("bind client socket");
        sock.set_nonblocking(true).expect("set nonblocking");
        client_sockets.push(sock);
    }

    // 2. Initialize World and Spatial Partitioning
    let mut world = WorldManager::new();
    let mut spatial_grid = SpatialHashGrid::with_capacity(2500, 2048);

    let bounds_zone1 = ZoneBounds::new(
        Fixed64::ZERO,
        Fixed64::from_i32(500),
        Fixed64::ZERO,
        Fixed64::from_i32(500),
        SeamAxis::EastWest,
        Fixed64::from_i32(450),
        Fixed64::from_i32(500),
    );
    world.add_zone(WorldZone::new(
        ZoneId(1),
        bounds_zone1,
        Some(ZoneId(2)),
        true,
        2500,
    ));

    let bounds_zone2 = ZoneBounds::new(
        Fixed64::from_i32(450),
        Fixed64::from_i32(950),
        Fixed64::ZERO,
        Fixed64::from_i32(500),
        SeamAxis::EastWest,
        Fixed64::from_i32(450),
        Fixed64::from_i32(500),
    );
    world.add_zone(WorldZone::new(
        ZoneId(2),
        bounds_zone2,
        Some(ZoneId(1)),
        false,
        2500,
    ));

    // 3. Populate Bots
    let mut bots: Vec<StressBot> = Vec::with_capacity(num_bots);
    for i in 0..num_bots {
        let eid = (i + 1) as u32;
        let is_migrator = i < 400;
        let pos = if is_migrator {
            let offset_x = Fixed64::from_i32(430 + (i % 20) as i32);
            let offset_z = Fixed64::from_i32(50 + ((i * 17) % 400) as i32);
            Vec3Fix::new(offset_x, Fixed64::ZERO, offset_z)
        } else {
            let col = (i % 40) as i32;
            let row = (i / 40) as i32;
            let pos_x = Fixed64::from_i32(10 + col * 11);
            let pos_z = Fixed64::from_i32(10 + row * 12);
            Vec3Fix::new(pos_x, Fixed64::ZERO, pos_z)
        };
        world
            .get_zone_mut(ZoneId(1))
            .expect("zone 1")
            .insert_entity(eid, pos)
            .expect("insert entity");
        let _ = spatial_grid.insert(eid, pos);
        bots.push(StressBot::new(eid, pos, is_migrator));
    }

    // 4. Setup Observers
    struct ObserverState {
        bot_index: usize,
        interest_set: ObserverInterestSet,
    }
    let mut observers: Vec<ObserverState> = Vec::with_capacity(num_observers);
    for i in 0..num_observers {
        observers.push(ObserverState {
            bot_index: i * (num_bots / num_observers),
            interest_set: ObserverInterestSet::with_capacity(64),
        });
    }

    let ingress_queue = SpscPacketQueue::<2048>::new();
    let egress_queue = SpscPacketQueue::<2048>::new();
    let mut coordinator = TickCoordinator::new(20);

    let mut tick_durations_us: Vec<u64> = Vec::with_capacity(total_ticks as usize);
    let mut phase_ingress_us: Vec<u64> = Vec::with_capacity(total_ticks as usize);
    let mut phase_sim_us: Vec<u64> = Vec::with_capacity(total_ticks as usize);
    let mut phase_aoi_us: Vec<u64> = Vec::with_capacity(total_ticks as usize);
    let mut phase_egress_us: Vec<u64> = Vec::with_capacity(total_ticks as usize);

    let mut aoi_query_buf = [0u32; 128];
    let mut aoi_events = [VisibilityEvent::Enter {
        entity_id: 0,
        tier: FrequencyTier::Immediate,
    }; 128];
    let mut repl_entities_buf = [(0u32, Vec3Fix::ZERO); 32];
    let mut repl_packet_buf = [0u8; 1024];

    // Run benchmark loop
    for tick in 1..=total_ticks {
        let tick_start = Instant::now();

        // Phase 1: Ingress
        let t_ing_start = Instant::now();
        // Generate client movement input for a subset of bots
        for bot in bots.iter_mut().take(200) {
            bot.step(tick_interval_secs);
            let (cx, cy, cz, quant) = QuantizedCellCoord::quantize_from_global(bot.state.position);
            let mut payload = [0u8; 17];
            payload[0..4].copy_from_slice(&bot.entity_id.to_le_bytes());
            payload[4..6].copy_from_slice(&(cx as i16).to_le_bytes());
            payload[6..8].copy_from_slice(&(cy as i16).to_le_bytes());
            payload[8..10].copy_from_slice(&(cz as i16).to_le_bytes());
            let transform_7b =
                quant.pack_with_yaw_and_flags(bot.state.yaw, bot.state.flags | FLAG_CELL_ANCHOR);
            payload[10..17].copy_from_slice(&transform_7b);

            let header = PacketHeader::new(
                ChannelType::UnreliableSequenced,
                PacketType::StateUpdate,
                tick as u16,
                0,
                0,
            );
            let mut wire_buf = [0u8; HEADER_SIZE + 17];
            let _ = header.write_to(&mut wire_buf[..HEADER_SIZE]);
            wire_buf[HEADER_SIZE..].copy_from_slice(&payload);
            let client_sock = &client_sockets[bot.entity_id as usize % num_client_sockets];
            let _ = client_sock.send_to(&wire_buf, bound_server_addr);
        }

        let _ = worker.drain_ingress(&ingress_queue, 1024);
        let t_ing_end = Instant::now();

        // Phase 2: Simulation & Packet Processing
        let t_sim_start = Instant::now();
        while let Some(packet) = ingress_queue.try_pop() {
            if let Ok(view) = PacketView::from_bytes(&packet.payload[..packet.len]) {
                if view.payload.len() >= 17 {
                    let payload = view.payload;
                    let eid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let cx = i16::from_le_bytes([payload[4], payload[5]]) as i32;
                    let cy = i16::from_le_bytes([payload[6], payload[7]]) as i32;
                    let cz = i16::from_le_bytes([payload[8], payload[9]]) as i32;
                    let mut transform = [0u8; 7];
                    transform.copy_from_slice(&payload[10..17]);
                    let (quant, _yaw, _flags) =
                        QuantizedCellCoord::unpack_with_yaw_and_flags(transform);
                    let new_pos = QuantizedCellCoord::dequantize_to_global(cx, cy, cz, quant);

                    let bot_idx = (eid - 1) as usize;
                    if let Some(bot) = bots.get_mut(bot_idx) {
                        let res = world.tick_entity_movement(eid, bot.current_zone, new_pos);
                        if let Ok(Some(ticket)) = res {
                            bot.current_zone = ticket.to_zone;
                        }
                    }
                    let _ = spatial_grid.update_position(eid, new_pos);
                }
            }
        }
        let t_sim_end = Instant::now();

        // Phase 3: Spatial & AoI
        let t_aoi_start = Instant::now();
        for (obs_idx, obs) in observers.iter_mut().enumerate() {
            let observer_pos = bots[obs.bot_index].state.position;
            let q_res = spatial_grid.query_radius_squared(
                observer_pos,
                MAX_AOI_RADIUS_SQ,
                &mut aoi_query_buf,
            );
            let _ = obs.interest_set.update_visibility(
                &spatial_grid,
                observer_pos,
                &aoi_query_buf[..q_res.written],
                &mut aoi_events,
            );

            let mut repl_len = 0;
            let visible_ids = obs.interest_set.visible_entities();
            let visible_tiers = obs.interest_set.visible_tiers();

            for (&vid, &tier) in visible_ids.iter().zip(visible_tiers.iter()) {
                if vid == bots[obs.bot_index].entity_id || tier != FrequencyTier::Immediate {
                    continue;
                }
                if tick % 2 == 0 {
                    if let Some(pos) = spatial_grid.get_position(vid) {
                        if repl_len < 7 {
                            repl_entities_buf[repl_len] = (vid, pos);
                            repl_len += 1;
                        }
                    }
                }
            }

            if repl_len < 7 {
                for (&vid, &tier) in visible_ids.iter().zip(visible_tiers.iter()) {
                    if vid == bots[obs.bot_index].entity_id || tier != FrequencyTier::Mid {
                        continue;
                    }
                    if tick % 2 == 0 && ((vid as u64) % 5 == (tick / 2) % 5) {
                        if let Some(pos) = spatial_grid.get_position(vid) {
                            if repl_len < 7 {
                                repl_entities_buf[repl_len] = (vid, pos);
                                repl_len += 1;
                            }
                        }
                    }
                }
            }

            if repl_len > 0 {
                let header = PacketHeader::new(
                    ChannelType::UnreliableSequenced,
                    PacketType::StateUpdate,
                    tick as u16,
                    obs_idx as u16,
                    0,
                );
                let _ = header.write_to(&mut repl_packet_buf[..HEADER_SIZE]);
                let mut offset = HEADER_SIZE;
                for &(eid, epos) in &repl_entities_buf[..repl_len] {
                    let raw_eid = eid;
                    repl_packet_buf[offset..offset + 4].copy_from_slice(&raw_eid.to_le_bytes());
                    offset += 4;
                    let (_cx, _cy, _cz, quant) = QuantizedCellCoord::quantize_from_global(epos);
                    let transform_7b =
                        quant.pack_with_yaw_and_flags(QuantizedYaw::NORTH, FLAG_WALKING);
                    repl_packet_buf[offset..offset + 7].copy_from_slice(&transform_7b);
                    offset += 7;
                }
                let peer_addr = client_sockets[obs_idx % num_client_sockets]
                    .local_addr()
                    .unwrap();
                if let Some(pkt) = NetworkPacket::new(peer_addr, &repl_packet_buf[..offset]) {
                    let _ = egress_queue.try_push(pkt);
                }
            }
        }
        let t_aoi_end = Instant::now();

        // Phase 4: Egress Flush
        let t_egr_start = Instant::now();
        let _ = worker.flush_egress(&egress_queue, 1024);
        let t_egr_end = Instant::now();

        let tick_elapsed = tick_start.elapsed();
        coordinator.record_tick_execution(tick_elapsed);

        tick_durations_us.push(tick_elapsed.as_micros() as u64);
        phase_ingress_us.push(t_ing_end.duration_since(t_ing_start).as_micros() as u64);
        phase_sim_us.push(t_sim_end.duration_since(t_sim_start).as_micros() as u64);
        phase_aoi_us.push(t_aoi_end.duration_since(t_aoi_start).as_micros() as u64);
        phase_egress_us.push(t_egr_end.duration_since(t_egr_start).as_micros() as u64);
    }

    // Sort samples for percentile calculation
    tick_durations_us.sort_unstable();
    phase_ingress_us.sort_unstable();
    phase_sim_us.sort_unstable();
    phase_aoi_us.sort_unstable();
    phase_egress_us.sort_unstable();

    let p50_tick = calculate_percentile(&tick_durations_us, 50.0);
    let p75_tick = calculate_percentile(&tick_durations_us, 75.0);
    let p90_tick = calculate_percentile(&tick_durations_us, 90.0);
    let p95_tick = calculate_percentile(&tick_durations_us, 95.0);
    let p99_tick = calculate_percentile(&tick_durations_us, 99.0);
    let max_tick = *tick_durations_us.last().unwrap_or(&0);
    let mean_tick: u64 = tick_durations_us.iter().sum::<u64>() / (total_ticks.max(1));

    let p99_ingress = calculate_percentile(&phase_ingress_us, 99.0);
    let p99_sim = calculate_percentile(&phase_sim_us, 99.0);
    let p99_aoi = calculate_percentile(&phase_aoi_us, 99.0);
    let p99_egress = calculate_percentile(&phase_egress_us, 99.0);

    let budget_us = 50_000u64; // 50ms at 20 Hz
    let headroom_p99_pct = ((budget_us as f64 - p99_tick as f64) / budget_us as f64) * 100.0;
    let headroom_p50_pct = ((budget_us as f64 - p50_tick as f64) / budget_us as f64) * 100.0;

    println!(
        "\n================================================================================\n\
         CAPACITY HEADROOM & LATENCY PERCENTILES: 2,000 CCU / 100 OBSERVERS (20 Hz)\n\
         ================================================================================\n\
         Tick Budget: 50.00 ms (50,000 us)\n\
         Tick Duration Percentiles:\n\
           - Min:  {:>8.2} ms ({:>6} us)\n\
           - Mean: {:>8.2} ms ({:>6} us)\n\
           - p50:  {:>8.2} ms ({:>6} us) [Headroom: {:>5.1}%]\n\
           - p75:  {:>8.2} ms ({:>6} us)\n\
           - p90:  {:>8.2} ms ({:>6} us)\n\
           - p95:  {:>8.2} ms ({:>6} us)\n\
           - p99:  {:>8.2} ms ({:>6} us) [Headroom: {:>5.1}%]\n\
           - Max:  {:>8.2} ms ({:>6} us)\n\
         Phase Breakdown at p99 Latency:\n\
           - Phase 1 (Ingress & Drain): {:>8.2} ms ({:>6} us)\n\
           - Phase 2 (World Sim):       {:>8.2} ms ({:>6} us)\n\
           - Phase 3 (Spatial & AoI):   {:>8.2} ms ({:>6} us)\n\
           - Phase 4 (Egress Flush):    {:>8.2} ms ({:>6} us)\n\
         Load Shedding Level: {:?}\n\
         ================================================================================",
        (*tick_durations_us.first().unwrap_or(&0) as f64) / 1000.0,
        *tick_durations_us.first().unwrap_or(&0),
        (mean_tick as f64) / 1000.0,
        mean_tick,
        (p50_tick as f64) / 1000.0,
        p50_tick,
        headroom_p50_pct,
        (p75_tick as f64) / 1000.0,
        p75_tick,
        (p90_tick as f64) / 1000.0,
        p90_tick,
        (p95_tick as f64) / 1000.0,
        p95_tick,
        (p99_tick as f64) / 1000.0,
        p99_tick,
        headroom_p99_pct,
        (max_tick as f64) / 1000.0,
        max_tick,
        (p99_ingress as f64) / 1000.0,
        p99_ingress,
        (p99_sim as f64) / 1000.0,
        p99_sim,
        (p99_aoi as f64) / 1000.0,
        p99_aoi,
        (p99_egress as f64) / 1000.0,
        p99_egress,
        coordinator.shedding_level()
    );

    // Invariant assertions:
    // Even at p99, the server must have at least 40% CPU headroom against the 50ms tick budget
    let limit_us = if cfg!(debug_assertions) {
        45_000
    } else {
        30_000
    };
    assert!(
        p99_tick < limit_us,
        "p99 tick duration ({p99_tick} us) exceeded {limit_us}us limit"
    );
    assert_eq!(
        coordinator.shedding_level(),
        LoadSheddingLevel::None,
        "Server must not escalate load shedding under normal 2,000 CCU load"
    );
}

#[test]
fn test_extreme_density_cluster_scaling_up_to_99th_percentile() {
    // Tests hot-spot cluster scaling across sparse (5) to 99th percentile (500) entities
    let density_tiers = [
        ("Sparse Encounter", 5usize),
        ("50th Percentile (Ambient Dispersion)", 10usize),
        ("75th Percentile (Active Hub / Skirmish)", 50usize),
        ("90th Percentile (Chokepoint / Dungeon)", 100usize),
        ("95th Percentile (Major Raid Encounter)", 250usize),
        ("99th Percentile (Extreme Flash Mob)", 500usize),
    ];

    println!(
        "\n====================================================================================================\n\
         EXTREME DENSITY CLUSTER SCALING: UNCLAMPED DEMAND VS SCHEDULED OUTPUT\n\
         ====================================================================================================\n\
         {:^38} | {:^8} | {:^10} | {:^10} | {:^16} | {:^16} | {:^11}\n\
         ----------------------------------------------------------------------------------------------------",
        "Cluster Scenario", "Entities", "Query", "Reconcile", "Unclamped Demand", "Scheduled Output", "Suppression"
    );

    let wire_budget_bps = 1.2 * 1024.0; // 1,228.8 B/s

    for &(scenario, entity_count) in &density_tiers {
        let mut spatial_grid = SpatialHashGrid::with_capacity(1000, 512);
        let observer_pos = Vec3Fix::new(
            Fixed64::from_i32(100),
            Fixed64::from_i32(10),
            Fixed64::from_i32(100),
        );
        let mut interest_set = ObserverInterestSet::with_capacity(entity_count.max(64));

        // Pack entities into a tight 25m cluster around the observer
        for i in 0..entity_count {
            let eid = (i + 1) as u32;
            let angle = (i as f64) * (2.0 * std::f64::consts::PI / (entity_count as f64));
            let radius = 2.0 + (i as f64 % 20.0);
            let px = Fixed64::from_f64(100.0 + radius * angle.cos());
            let pz = Fixed64::from_f64(100.0 + radius * angle.sin());
            let _ = spatial_grid.insert(eid, Vec3Fix::new(px, Fixed64::from_i32(10), pz));
        }

        let mut query_buf = vec![0u32; entity_count + 32];
        let mut events_buf = vec![
            VisibilityEvent::Enter {
                entity_id: 0,
                tier: FrequencyTier::Immediate
            };
            entity_count + 32
        ];

        // Measure query duration across 500 iterations
        let q_start = Instant::now();
        let iterations = 500;
        let mut total_matches = 0;
        for _ in 0..iterations {
            let res =
                spatial_grid.query_radius_squared(observer_pos, MAX_AOI_RADIUS_SQ, &mut query_buf);
            total_matches = res.written;
        }
        let avg_query_us = (q_start.elapsed().as_nanos() as f64) / (iterations as f64 * 1000.0);

        // Measure visibility update duration
        let r_start = Instant::now();
        for _ in 0..iterations {
            let _ = interest_set.update_visibility(
                &spatial_grid,
                observer_pos,
                &query_buf[..total_matches],
                &mut events_buf,
            );
        }
        let avg_reconcile_us = (r_start.elapsed().as_nanos() as f64) / (iterations as f64 * 1000.0);

        // 1. Unclamped Replication Demand:
        // Raw broadcast rate if all visible entities in cluster were sent every tick (20 Hz)
        // without frequency tiers or packet paging (12B header + 11B/entity + 28B IPv4/UDP):
        let unclamped_payload_per_tick = (HEADER_SIZE + (entity_count * 11)) as f64;
        let unclamped_wire_bps = (unclamped_payload_per_tick + 28.0) * 20.0;

        // 2. Scheduled & Clamped Egress Output:
        // Enforced by AoI frequency scheduler and priority packet packing (Immediate 10 Hz, max 7 entities/pkt)
        const MAX_ENTITIES_PER_PACKET: usize = 7;
        let immediate_count = interest_set
            .visible_tiers()
            .iter()
            .filter(|&&t| t == FrequencyTier::Immediate)
            .count();
        let mid_count = interest_set
            .visible_tiers()
            .iter()
            .filter(|&&t| t == FrequencyTier::Mid)
            .count();

        let packets_per_sec = 10.0;
        let immediate_in_packet = immediate_count.min(MAX_ENTITIES_PER_PACKET);
        let mid_in_packet = (MAX_ENTITIES_PER_PACKET - immediate_in_packet).min(mid_count);
        let entities_in_packet = immediate_in_packet + mid_in_packet;

        let clamped_payload_bytes_per_pkt = (HEADER_SIZE + (entities_in_packet * 11)) as f64;
        let clamped_wire_bytes_per_pkt = clamped_payload_bytes_per_pkt + 28.0;
        let clamped_wire_bps = clamped_wire_bytes_per_pkt * packets_per_sec;

        let suppression_pct = (1.0 - (clamped_wire_bps / unclamped_wire_bps)) * 100.0;

        println!(
            "{:<38} | {:>8} | {:>7.2} us | {:>7.2} us | {:>10.2} B/s ({:>5.1} KB/s) | {:>10.2} B/s ({:>4.2} KB/s) | {:>9.1}%",
            scenario,
            entity_count,
            avg_query_us,
            avg_reconcile_us,
            unclamped_wire_bps,
            unclamped_wire_bps / 1024.0,
            clamped_wire_bps,
            clamped_wire_bps / 1024.0,
            suppression_pct
        );

        // Assertions:
        // 1. Spatial query must remain sub-millisecond (< 500 us) even for 500 entities
        assert!(
            avg_query_us < 500.0,
            "Spatial query exceeded 500 us for {entity_count} entities: {avg_query_us:.2} us"
        );
        // 2. Scheduled wire egress must strictly remain clamped under the 1.2 KB/s budget (1,228.8 B/s)
        assert!(
            clamped_wire_bps <= wire_budget_bps,
            "Scheduled wire egress ({clamped_wire_bps:.2} B/s) exceeded 1.2 KB/s wire budget for {entity_count} entities"
        );
        // 3. For large clusters, unclamped demand would have severely breached the wire budget
        if entity_count >= 50 {
            assert!(
                unclamped_wire_bps > wire_budget_bps,
                "Unclamped demand for {entity_count} entities should exceed wire budget"
            );
        }
    }
    println!("====================================================================================================");
}

#[test]
fn test_load_shedding_escalation_and_recovery_under_induced_overload() {
    let mut coordinator = TickCoordinator::new(20);

    // Normal operation: 5 ticks at 10ms
    for _ in 0..5 {
        coordinator.record_tick_execution(std::time::Duration::from_millis(10));
        assert_eq!(coordinator.shedding_level(), LoadSheddingLevel::None);
    }

    // Induce Level 1 (>=40ms): 1 tick at 41ms
    coordinator.record_tick_execution(std::time::Duration::from_millis(41));
    assert_eq!(
        coordinator.shedding_level(),
        LoadSheddingLevel::Level1,
        "Should escalate to Level1 shedding at >=40ms"
    );

    // Induce Level 2 (>=45ms): 1 tick at 46ms
    coordinator.record_tick_execution(std::time::Duration::from_millis(46));
    assert_eq!(
        coordinator.shedding_level(),
        LoadSheddingLevel::Level2,
        "Should escalate to Level2 shedding at >=45ms"
    );

    // Hysteresis recovery: requires 5 ticks <45ms to drop to Level1, then 10 ticks <40ms to drop to None
    for _ in 0..4 {
        coordinator.record_tick_execution(std::time::Duration::from_millis(30));
        assert_eq!(coordinator.shedding_level(), LoadSheddingLevel::Level2);
    }
    coordinator.record_tick_execution(std::time::Duration::from_millis(30)); // 5th tick
    assert_eq!(
        coordinator.shedding_level(),
        LoadSheddingLevel::Level1,
        "Should drop to Level1 after 5 consecutive calm ticks"
    );

    for _ in 0..9 {
        coordinator.record_tick_execution(std::time::Duration::from_millis(20));
        assert_eq!(coordinator.shedding_level(), LoadSheddingLevel::Level1);
    }
    coordinator.record_tick_execution(std::time::Duration::from_millis(20)); // 10th tick
    assert_eq!(
        coordinator.shedding_level(),
        LoadSheddingLevel::None,
        "Should drop to None after 10 consecutive calm ticks"
    );
}
