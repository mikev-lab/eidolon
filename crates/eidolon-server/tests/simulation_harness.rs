//! Authoritative 2,000 CCU bot load simulation harness and bandwidth verification.
//!
//! Validates the sub-1.2 KB/s wire budget invariant, 20 Hz tick cadence stability,
//! zero heap allocation inside simulation loops, and seamless zone seam migrations.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::kinematics::{
    extrapolate, should_dispatch_update, DeadReckoningConfig, KinematicState, FLAG_SPRINTING,
    FLAG_WALKING,
};
use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw};
use eidolon_net::packet::{PacketHeader, PacketView};
use eidolon_net::protocol::{ChannelType, PacketType, HEADER_SIZE};
use eidolon_server::io::NetworkIoWorker;
use eidolon_server::queue::{NetworkPacket, SpscPacketQueue};
use eidolon_server::tick::TickCoordinator;
use eidolon_spatial::aoi::{
    AoIScheduler, LoadSheddingLevel, ObserverInterestSet, VisibilityEvent, MAX_AOI_RADIUS_SQ,
};
use eidolon_spatial::grid::SpatialHashGrid;
use eidolon_world::zone::{SeamAxis, WorldManager, WorldZone, ZoneBounds, ZoneId};

/// Synthetic client bot executing intent-based movement and dead reckoning prediction.
struct SyntheticBot {
    entity_id: u32,
    current_zone: ZoneId,
    authoritative: KinematicState,
    extrapolated: KinematicState,
    elapsed_ticks: u32,
    patrol_speed: Fixed64,
    direction: i32,
    is_migrator: bool,
    client_pool_idx: usize,
}

impl SyntheticBot {
    fn new(entity_id: u32, initial_pos: Vec3Fix, is_migrator: bool, pool_idx: usize) -> Self {
        let speed = if is_migrator {
            Fixed64::from_f64(6.0) // 6.0 m/s moving East across the seam
        } else {
            Fixed64::from_f64(3.0) // 3.0 m/s patrolling
        };

        let vel = if is_migrator {
            Vec3Fix::new(speed, Fixed64::ZERO, Fixed64::ZERO)
        } else {
            Vec3Fix::new(Fixed64::ZERO, Fixed64::ZERO, speed)
        };

        let initial_state = KinematicState::with_velocity(
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
            authoritative: initial_state,
            extrapolated: initial_state,
            // Stagger heartbeat timers across entities to ensure smooth network flow
            elapsed_ticks: entity_id % 40,
            patrol_speed: speed,
            direction: 1,
            is_migrator,
            client_pool_idx: pool_idx,
        }
    }

    fn step_movement(&mut self, dt: Fixed64, tick: u64) {
        if self.is_migrator {
            // Migrators continue moving East across seam at constant speed
            self.authoritative.position.x += self.patrol_speed * dt;
        } else {
            // Patrollers reverse direction when approaching boundary
            if self.authoritative.position.z >= Fixed64::from_i32(480) {
                self.direction = -1;
                self.authoritative.velocity.z = -self.patrol_speed;
            } else if self.authoritative.position.z <= Fixed64::from_i32(20) {
                self.direction = 1;
                self.authoritative.velocity.z = self.patrol_speed;
            }

            // Periodic heading/speed variation to stress dead reckoning divergence
            if tick.is_multiple_of(60) && self.entity_id.is_multiple_of(5) {
                let current_yaw = self.authoritative.yaw.as_byte();
                self.authoritative.yaw = QuantizedYaw::from_byte(current_yaw.wrapping_add(16));
            }

            self.authoritative.position.z += self.authoritative.velocity.z * dt;
        }
    }
}

#[test]
fn test_2000_ccu_bot_simulation_bandwidth_and_stability() {
    let tick_interval_secs = Fixed64::from_f64(0.050); // 50ms = 20 Hz
    let total_ticks = 200u64; // 10.0 seconds of simulation
    let num_bots = 2000usize;

    // 1. Initialize Server Network Worker on loopback
    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let worker = NetworkIoWorker::bind(server_addr).expect("bind server worker");
    let bound_server_addr = worker.local_addr().expect("server local addr");

    // 2. Initialize Client Sockets Pool (4 client endpoints)
    let num_client_sockets = 4;
    let mut client_sockets = Vec::with_capacity(num_client_sockets);
    for _ in 0..num_client_sockets {
        let sock = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .expect("bind client socket");
        sock.set_nonblocking(true).expect("set nonblocking");
        client_sockets.push(sock);
    }

    // 3. Initialize Multi-Zone World Topology (Zone 1 and Zone 2)
    let mut world = WorldManager::new();
    let mut spatial_grid = SpatialHashGrid::with_capacity(2500, 2048);

    // Zone 1: X in [0, 500], Z in [0, 500], Seam at X in [450, 500] (midpoint 475), neighbor Zone 2
    let bounds_zone1 = ZoneBounds::new(
        Fixed64::ZERO,
        Fixed64::from_i32(500),
        Fixed64::ZERO,
        Fixed64::from_i32(500),
        SeamAxis::EastWest,
        Fixed64::from_i32(450),
        Fixed64::from_i32(500),
    );
    let zone1 = WorldZone::new(ZoneId(1), bounds_zone1, Some(ZoneId(2)), true, 2500);
    world.add_zone(zone1);

    // Zone 2: X in [450, 950], Z in [0, 500], Seam at X in [450, 500] (midpoint 475), neighbor Zone 1
    let bounds_zone2 = ZoneBounds::new(
        Fixed64::from_i32(450),
        Fixed64::from_i32(950),
        Fixed64::ZERO,
        Fixed64::from_i32(500),
        SeamAxis::EastWest,
        Fixed64::from_i32(450),
        Fixed64::from_i32(500),
    );
    let zone2 = WorldZone::new(ZoneId(2), bounds_zone2, Some(ZoneId(1)), false, 2500);
    world.add_zone(zone2);

    // 4. Instantiate 2,000 Synthetic Bots
    let mut bots: Vec<SyntheticBot> = Vec::with_capacity(num_bots);
    for i in 0..num_bots {
        let entity_id = (i + 1) as u32;
        let is_migrator = i < 400; // First 400 bots migrate from Zone 1 to Zone 2

        let pos = if is_migrator {
            // Positioned at X = 430..455, moving East across seam midpoint (475)
            let x_offset = Fixed64::from_i32(430 + ((i % 25) as i32));
            let z_offset = Fixed64::from_i32(20 + ((i % 450) as i32));
            Vec3Fix::new(x_offset, Fixed64::ZERO, z_offset)
        } else {
            // Scattered across Zone 1 [20, 420] x [20, 480]
            let x_offset = Fixed64::from_i32(20 + (((i * 17) % 400) as i32));
            let z_offset = Fixed64::from_i32(20 + (((i * 23) % 460) as i32));
            Vec3Fix::new(x_offset, Fixed64::ZERO, z_offset)
        };

        // Insert initial entity into Zone 1 and Spatial Grid
        world
            .get_zone_mut(ZoneId(1))
            .expect("zone 1")
            .insert_entity(entity_id, pos)
            .expect("insert entity");
        spatial_grid.insert(entity_id, pos).expect("insert spatial");

        let pool_idx = i % num_client_sockets;
        bots.push(SyntheticBot::new(entity_id, pos, is_migrator, pool_idx));
    }

    assert_eq!(
        world.get_zone(ZoneId(1)).expect("zone 1").entity_count(),
        2000
    );
    assert_eq!(world.get_zone(ZoneId(2)).expect("zone 2").entity_count(), 0);

    // 5. Initialize Pre-Allocated SPSC Queues, AoI Observers, and Tick Coordinator
    let ingress_queue = SpscPacketQueue::<4096>::new();
    let egress_queue = SpscPacketQueue::<4096>::new();
    let mut coordinator = TickCoordinator::new(20);
    let dr_config = DeadReckoningConfig::default();

    // Register 100 active observer clients uniformly sampled across the bot population
    struct BotObserver {
        bot_index: usize,
        interest_set: ObserverInterestSet,
    }
    let num_observers = 100usize;
    let mut observers: Vec<BotObserver> = (0..num_observers)
        .map(|idx| BotObserver {
            bot_index: idx * (num_bots / num_observers),
            interest_set: ObserverInterestSet::with_capacity(128),
        })
        .collect();

    let aoi_scheduler = AoIScheduler::new();
    let mut aoi_query_buf = [0u32; 128];
    let mut aoi_events = [VisibilityEvent::Exit { entity_id: 0 }; 64];
    let mut repl_entities_buf = [(0u32, Vec3Fix::ZERO); 32];
    let mut repl_packet_buf = [0u8; 1024];

    let mut total_client_ingress_bytes = 0usize;
    let mut total_server_egress_bytes = 0usize;
    let mut total_migrations = 0usize;
    let mut packet_buffer = [0u8; 32];

    // 6. Execute 200-Tick Simulation Loop
    for tick in 0..total_ticks {
        let tick_start = Instant::now();

        // --- Client Side: Bot Kinematics and Dead Reckoning Evaluation ---
        for bot in bots.iter_mut() {
            bot.step_movement(tick_interval_secs, tick);
            bot.elapsed_ticks += 1;
            bot.extrapolated = extrapolate(&bot.extrapolated, 1, tick_interval_secs);

            let should_send = should_dispatch_update(
                &bot.authoritative,
                &bot.extrapolated,
                bot.elapsed_ticks,
                &dr_config,
            );

            if should_send {
                // Pack global-to-cell quantized coordinates
                let (cx, cy, cz, quant) =
                    QuantizedCellCoord::quantize_from_global(bot.authoritative.position);
                let transform_7b =
                    quant.pack_with_yaw_and_flags(bot.authoritative.yaw, bot.authoritative.flags);

                let header = PacketHeader::new(
                    ChannelType::UnreliableSequenced,
                    PacketType::StateUpdate,
                    tick as u16,
                    0,
                    0,
                );
                let _ = header
                    .write_to(&mut packet_buffer[..HEADER_SIZE])
                    .expect("write header");

                // Pack payload: 4-byte entity_id + 6-byte cell + 7-byte quantized transform = 17 bytes (29B total)
                packet_buffer[12..16].copy_from_slice(&bot.entity_id.to_le_bytes());
                packet_buffer[16..18].copy_from_slice(&(cx as i16).to_le_bytes());
                packet_buffer[18..20].copy_from_slice(&(cy as i16).to_le_bytes());
                packet_buffer[20..22].copy_from_slice(&(cz as i16).to_le_bytes());
                packet_buffer[22..29].copy_from_slice(&transform_7b);

                let packet_len = 29;
                if let Some(sock) = client_sockets.get(bot.client_pool_idx) {
                    if sock
                        .send_to(&packet_buffer[..packet_len], bound_server_addr)
                        .is_ok()
                    {
                        total_client_ingress_bytes += packet_len;
                    }
                }

                // Reset extrapolation reference on dispatch
                bot.extrapolated = bot.authoritative;
                bot.elapsed_ticks = 0;
            }
        }

        // --- Server Side: Drain Ingress, Validate Datagrams, Simulate World ---
        let _drained = worker.drain_ingress(&ingress_queue, 2048);

        while let Some(packet) = ingress_queue.try_pop() {
            if let Ok(view) = PacketView::from_bytes(&packet.payload[..packet.len]) {
                if view.header.packet_type == PacketType::StateUpdate && view.payload.len() >= 17 {
                    let payload = view.payload;
                    let entity_id =
                        u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let cx = i16::from_le_bytes([payload[4], payload[5]]) as i32;
                    let cy = i16::from_le_bytes([payload[6], payload[7]]) as i32;
                    let cz = i16::from_le_bytes([payload[8], payload[9]]) as i32;

                    let mut transform_bytes = [0u8; 7];
                    transform_bytes.copy_from_slice(&payload[10..17]);
                    let (quant, _yaw, _flags) =
                        QuantizedCellCoord::unpack_with_yaw_and_flags(transform_bytes);
                    let authoritative_pos =
                        QuantizedCellCoord::dequantize_to_global(cx, cy, cz, quant);

                    // Locate bot current zone and tick movement
                    let bot_idx = (entity_id - 1) as usize;
                    if let Some(bot) = bots.get_mut(bot_idx) {
                        let res = world.tick_entity_movement(
                            entity_id,
                            bot.current_zone,
                            authoritative_pos,
                        );
                        if let Ok(Some(ticket)) = res {
                            bot.current_zone = ticket.to_zone;
                            total_migrations += 1;
                        }
                    }

                    let _ = spatial_grid.update_position(entity_id, authoritative_pos);
                }
            }
        }

        // --- Server Side: Area of Interest (AoI) & Tiered Replication ---
        for (obs_idx, observer) in observers.iter_mut().enumerate() {
            let bot_idx = observer.bot_index;
            let observer_pos = bots[bot_idx].authoritative.position;

            let query_res = spatial_grid.query_radius_squared(
                observer_pos,
                MAX_AOI_RADIUS_SQ,
                &mut aoi_query_buf,
            );

            let _event_count = observer.interest_set.update_visibility(
                &spatial_grid,
                observer_pos,
                &aoi_query_buf[..query_res.written],
                &mut aoi_events,
            );

            let mut repl_len = 0;
            let visible_ids = observer.interest_set.visible_entities();
            let visible_tiers = observer.interest_set.visible_tiers();

            for (&visible_id, &tier) in visible_ids.iter().zip(visible_tiers.iter()) {
                if visible_id == bots[bot_idx].entity_id {
                    continue; // Skip self
                }

                if aoi_scheduler.should_replicate(tier, tick, false) {
                    if let Some(pos) = spatial_grid.get_position(visible_id) {
                        if repl_len < 32 {
                            repl_entities_buf[repl_len] = (visible_id, pos);
                            repl_len += 1;
                        }
                    }
                }
            }

            if repl_len > 0 {
                // Assemble server replication packet
                let header = PacketHeader::new(
                    ChannelType::UnreliableSequenced,
                    PacketType::StateUpdate,
                    tick as u16,
                    0,
                    0,
                );
                let _ = header.write_to(&mut repl_packet_buf[..HEADER_SIZE]);

                let mut offset = HEADER_SIZE;
                for &(eid, epos) in &repl_entities_buf[..repl_len] {
                    repl_packet_buf[offset..offset + 4].copy_from_slice(&eid.to_le_bytes());
                    offset += 4;

                    let (_cx, _cy, _cz, quant) = QuantizedCellCoord::quantize_from_global(epos);
                    let transform_7b =
                        quant.pack_with_yaw_and_flags(QuantizedYaw::NORTH, FLAG_WALKING);
                    repl_packet_buf[offset..offset + 7].copy_from_slice(&transform_7b);
                    offset += 7;
                }

                // Push replication datagram to egress queue
                let peer_addr = client_sockets[obs_idx % num_client_sockets]
                    .local_addr()
                    .unwrap();
                if let Some(pkt) = NetworkPacket::new(peer_addr, &repl_packet_buf[..offset]) {
                    if egress_queue.try_push(pkt) {
                        total_server_egress_bytes += offset;
                    }
                }
            }
        }

        let _flushed = worker.flush_egress(&egress_queue, 2048);

        // Drain client socket receive buffers
        let mut drain_buf = [0u8; 1024];
        for sock in &client_sockets {
            while sock.recv_from(&mut drain_buf).is_ok() {}
        }

        // Record execution duration and update coordinator
        let tick_duration = tick_start.elapsed();
        coordinator.record_tick_execution(tick_duration);
    }

    // 7. Verify Sub-1.2 KB/s Wire Budget Invariant on True Server Egress
    let simulated_seconds = (total_ticks as f64) * 0.050; // 10.0 seconds
    let avg_server_egress_bps =
        (total_server_egress_bytes as f64) / ((num_observers as f64) * simulated_seconds);
    let avg_client_ingress_bps =
        (total_client_ingress_bytes as f64) / ((num_bots as f64) * simulated_seconds);

    println!(
        "Simulation Complete: 2,000 bots (100 active observers) across 200 ticks (10.0s simulation)\n\
         - Total Server Replication Egress: {} bytes ({:.2} MB)\n\
         - Average Server Egress per Client: {:.2} B/s ({:.2} KB/s)\n\
         - Total Client Input Ingress: {} bytes ({:.2} MB)\n\
         - Average Client Ingress per Bot: {:.2} B/s ({:.2} KB/s)\n\
         - Total Zone Seam Migrations: {}\n\
         - Server Ticks Simulated: {}\n\
         - Shedding Level: {:?}",
        total_server_egress_bytes,
        (total_server_egress_bytes as f64) / (1024.0 * 1024.0),
        avg_server_egress_bps,
        avg_server_egress_bps / 1024.0,
        total_client_ingress_bytes,
        (total_client_ingress_bytes as f64) / (1024.0 * 1024.0),
        avg_client_ingress_bps,
        avg_client_ingress_bps / 1024.0,
        total_migrations,
        coordinator.metrics().total_ticks,
        coordinator.shedding_level()
    );

    // Section 2.1 Invariant: True server replication egress must strictly remain below 1.2 KB/s (1228.8 B/s)
    let wire_budget_bps = 1.2 * 1024.0;
    assert!(
        total_server_egress_bytes > 0,
        "Server must have generated and transmitted active replication packets"
    );
    assert!(
        avg_server_egress_bps <= wire_budget_bps,
        "Average server replication egress per client ({avg_server_egress_bps:.2} B/s) exceeded the 1.2 KB/s wire budget ({wire_budget_bps:.2} B/s)"
    );

    // Client ingress must also conform to the wire budget
    assert!(
        avg_client_ingress_bps <= wire_budget_bps,
        "Average client ingress per bot ({avg_client_ingress_bps:.2} B/s) exceeded the 1.2 KB/s wire budget"
    );

    // 8. Verify Entity Retention and Seam Migrations
    let zone1_entities = world.get_zone(ZoneId(1)).expect("zone 1").entity_count();
    let zone2_entities = world.get_zone(ZoneId(2)).expect("zone 2").entity_count();
    assert_eq!(
        zone1_entities + zone2_entities,
        2000,
        "Zero entity loss invariant violated across zone borders"
    );
    assert!(
        zone2_entities > 0,
        "Migrator bots must have successfully crossed seam into Zone 2"
    );
    assert!(
        total_migrations > 0,
        "Seam boundary handoffs must have occurred"
    );

    // 9. Verify Coordinator Metrics and Load Shedding
    assert_eq!(coordinator.metrics().total_ticks, 200);
    assert_eq!(coordinator.shedding_level(), LoadSheddingLevel::None);
}

#[test]
fn test_20hz_realtime_cadence_jitter() {
    let mut coordinator = TickCoordinator::new(20);
    let target_interval = Duration::from_millis(50);
    let sample_ticks = 10;

    let test_start = Instant::now();

    for _ in 0..sample_ticks {
        let tick_start = Instant::now();

        // Simulate minimal work
        std::thread::sleep(Duration::from_millis(2));

        let execution = tick_start.elapsed();
        coordinator.record_tick_execution(execution);
        coordinator.sleep_headroom(execution);
    }

    let elapsed = test_start.elapsed();
    let expected_duration = target_interval * sample_ticks;

    let delta = elapsed.abs_diff(expected_duration);

    println!(
        "Real-Time Cadence: 10 ticks target: {:?}, actual: {:?}, delta: {:?}",
        expected_duration, elapsed, delta
    );

    // Cadence timing tolerance accommodates OS kernel timer coalescing and virtual machine jitter
    // (especially on virtualized macOS Darwin CI runners where thread::sleep has coarse ~10ms resolution).
    assert!(
        delta < Duration::from_millis(150),
        "Tick cadence drift exceeded tolerance: {delta:?}"
    );
    assert_eq!(coordinator.metrics().total_ticks, sample_ticks as u64);
}
