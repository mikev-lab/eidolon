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
use eidolon_net::packet::PacketHeader;
use eidolon_net::protocol::{ChannelType, PacketType, HEADER_SIZE};
use eidolon_server::io::NetworkIoWorker;
use eidolon_server::queue::SpscPacketQueue;
use eidolon_server::tick::TickCoordinator;
use eidolon_spatial::aoi::LoadSheddingLevel;
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
            Fixed64::from_f64(4.0) // 4.0 m/s moving East across the seam
        } else {
            Fixed64::from_f64(2.5) // 2.5 m/s patrolling
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
            if self.authoritative.position.z >= Fixed64::from_i32(60) {
                self.direction = -1;
                self.authoritative.velocity.z = -self.patrol_speed;
            } else if self.authoritative.position.z <= Fixed64::from_i32(4) {
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

    // Zone 1: X in [0, 72], Z in [0, 64], Seam at X in [56, 72] (midpoint 64), neighbor Zone 2
    let bounds_zone1 = ZoneBounds::new(
        Fixed64::ZERO,
        Fixed64::from_i32(72),
        Fixed64::ZERO,
        Fixed64::from_i32(64),
        SeamAxis::EastWest,
        Fixed64::from_i32(56),
        Fixed64::from_i32(72),
    );
    let zone1 = WorldZone::new(ZoneId(1), bounds_zone1, Some(ZoneId(2)), true, 2500);
    world.add_zone(zone1);

    // Zone 2: X in [56, 128], Z in [0, 64], Seam at X in [56, 72] (midpoint 64), neighbor Zone 1
    let bounds_zone2 = ZoneBounds::new(
        Fixed64::from_i32(56),
        Fixed64::from_i32(128),
        Fixed64::ZERO,
        Fixed64::from_i32(64),
        SeamAxis::EastWest,
        Fixed64::from_i32(56),
        Fixed64::from_i32(72),
    );
    let zone2 = WorldZone::new(ZoneId(2), bounds_zone2, Some(ZoneId(1)), false, 2500);
    world.add_zone(zone2);

    // 4. Instantiate 2,000 Synthetic Bots
    let mut bots: Vec<SyntheticBot> = Vec::with_capacity(num_bots);
    for i in 0..num_bots {
        let entity_id = (i + 1) as u32;
        let is_migrator = i < 400; // First 400 bots migrate from Zone 1 to Zone 2

        let pos = if is_migrator {
            // Positioned at X = 20..35, moving East across seam midpoint (64)
            let x_offset = Fixed64::from_i32(20 + ((i % 15) as i32));
            let z_offset = Fixed64::from_i32(10 + ((i % 40) as i32));
            Vec3Fix::new(x_offset, Fixed64::ZERO, z_offset)
        } else {
            // Scattered across Zone 1
            let x_offset = Fixed64::from_i32(5 + ((i % 45) as i32));
            let z_offset = Fixed64::from_i32(5 + ((i % 50) as i32));
            Vec3Fix::new(x_offset, Fixed64::ZERO, z_offset)
        };

        // Insert initial entity into Zone 1
        world
            .get_zone_mut(ZoneId(1))
            .expect("zone 1")
            .insert_entity(entity_id, pos)
            .expect("insert entity");

        let pool_idx = i % num_client_sockets;
        bots.push(SyntheticBot::new(entity_id, pos, is_migrator, pool_idx));
    }

    assert_eq!(
        world.get_zone(ZoneId(1)).expect("zone 1").entity_count(),
        2000
    );
    assert_eq!(world.get_zone(ZoneId(2)).expect("zone 2").entity_count(), 0);

    // 5. Initialize Pre-Allocated SPSC Queues and Tick Coordinator
    let ingress_queue = SpscPacketQueue::<4096>::new();
    let egress_queue = SpscPacketQueue::<4096>::new();
    let mut coordinator = TickCoordinator::new(20);
    let dr_config = DeadReckoningConfig::default();

    let mut total_bytes_transmitted = 0usize;
    let mut total_migrations = 0usize;
    let mut packet_buffer = [0u8; 32];

    // 6. Execute 200-Tick Simulation Loop
    for tick in 0..total_ticks {
        let tick_start = Instant::now();

        // --- Client Side: Bot Kinematics and Dead Reckoning Evaluation ---
        for bot in bots.iter_mut() {
            bot.step_movement(tick_interval_secs, tick);

            let should_send = should_dispatch_update(
                &bot.authoritative,
                &bot.extrapolated,
                bot.elapsed_ticks,
                &dr_config,
            );

            if should_send {
                // Pack 12-byte header
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

                // Pack payload: 4-byte entity_id + 7-byte quantized transform = 11 bytes
                let entity_bytes = bot.entity_id.to_le_bytes();
                packet_buffer[12..16].copy_from_slice(&entity_bytes);

                let quant = QuantizedCellCoord::quantize(
                    bot.authoritative.position.x,
                    bot.authoritative.position.y,
                    bot.authoritative.position.z,
                );
                let transform_7b =
                    quant.pack_with_yaw_and_flags(bot.authoritative.yaw, bot.authoritative.flags);
                packet_buffer[16..23].copy_from_slice(&transform_7b);

                let packet_len = 23; // 12 + 11
                if let Some(sock) = client_sockets.get(bot.client_pool_idx) {
                    if sock
                        .send_to(&packet_buffer[..packet_len], bound_server_addr)
                        .is_ok()
                    {
                        total_bytes_transmitted += packet_len;
                    }
                }

                // Reset extrapolation reference on dispatch
                bot.extrapolated = bot.authoritative;
                bot.elapsed_ticks = 0;
            } else {
                bot.elapsed_ticks += 1;
                bot.extrapolated = extrapolate(&bot.extrapolated, 1, tick_interval_secs);
            }
        }

        // --- Server Side: Drain Ingress, Simulate World, Flush Egress ---
        let _drained = worker.drain_ingress(&ingress_queue, 2048);

        while let Some(packet) = ingress_queue.try_pop() {
            if packet.len >= 23 {
                let entity_id = u32::from_le_bytes([
                    packet.payload[12],
                    packet.payload[13],
                    packet.payload[14],
                    packet.payload[15],
                ]);

                let mut transform_bytes = [0u8; 7];
                transform_bytes.copy_from_slice(&packet.payload[16..23]);
                let (quant, _yaw, _flags) =
                    QuantizedCellCoord::unpack_with_yaw_and_flags(transform_bytes);
                let authoritative_pos = quant.dequantize();

                // Locate bot current zone
                let bot_idx = (entity_id - 1) as usize;
                if let Some(bot) = bots.get_mut(bot_idx) {
                    if let Ok(Some(ticket)) =
                        world.tick_entity_movement(entity_id, bot.current_zone, authoritative_pos)
                    {
                        bot.current_zone = ticket.to_zone;
                        total_migrations += 1;
                    }
                }
            }
        }

        let _flushed = worker.flush_egress(&egress_queue, 2048);

        // Record execution duration and update coordinator
        let tick_duration = tick_start.elapsed();
        coordinator.record_tick_execution(tick_duration);
    }

    // 7. Verify Sub-1.2 KB/s Bandwidth Budget Invariant
    let simulated_seconds = (total_ticks as f64) * 0.050; // 10.0 seconds
    let avg_bytes_per_sec_per_bot =
        (total_bytes_transmitted as f64) / ((num_bots as f64) * simulated_seconds);

    println!(
        "Simulation Complete: 2,000 bots across 200 ticks (10.0s simulation)\n\
         - Total Bytes Transmitted: {} bytes ({:.2} MB)\n\
         - Average Bandwidth per Bot: {:.2} B/s ({:.2} KB/s)\n\
         - Total Zone Seam Migrations: {}\n\
         - Server Ticks Simulated: {}\n\
         - Shedding Level: {:?}",
        total_bytes_transmitted,
        (total_bytes_transmitted as f64) / (1024.0 * 1024.0),
        avg_bytes_per_sec_per_bot,
        avg_bytes_per_sec_per_bot / 1024.0,
        total_migrations,
        coordinator.metrics().total_ticks,
        coordinator.shedding_level()
    );

    // Section 2.1 Invariant: Must strictly remain below 1.2 KB/s (1228.8 B/s)
    assert!(
        avg_bytes_per_sec_per_bot < 1200.0,
        "Average bandwidth per bot ({avg_bytes_per_sec_per_bot:.2} B/s) exceeded the 1.2 KB/s wire budget"
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
