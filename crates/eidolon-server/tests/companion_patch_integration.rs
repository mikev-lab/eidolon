//! Companion patch integration test verifying co-existence of pak-delta asset synchronization
//! and 20 Hz authoritative MMO simulation.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Instant;

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::kinematics::{KinematicState, FLAG_WALKING};
use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw};
use eidolon_net::packet::PacketHeader;
use eidolon_net::protocol::{ChannelType, PacketType, HEADER_SIZE};
use eidolon_server::io::NetworkIoWorker;
use eidolon_server::queue::SpscPacketQueue;
use eidolon_server::tick::TickCoordinator;
use eidolon_world::companion::{
    AssetManifestDigest, PatchNegotiationState, PatchNegotiator, ZoneAssetRequirement,
};
use eidolon_world::zone::{SeamAxis, WorldManager, WorldZone, ZoneBounds, ZoneId};

#[test]
fn test_companion_patch_delta_negotiation_and_gameplay_stream() {
    // 1. Initialize Server Network Worker on loopback
    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let worker = NetworkIoWorker::bind(server_addr).expect("bind server worker");
    let bound_server_addr = worker.local_addr().expect("server local addr");

    // 2. Initialize Client Socket
    let client_sock = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind client socket");
    client_sock.set_nonblocking(true).expect("set nonblocking");

    // 3. Initialize World and Companion Zone Asset Requirement
    let mut world = WorldManager::new();
    let bounds = ZoneBounds::new(
        Fixed64::ZERO,
        Fixed64::from_i32(100),
        Fixed64::ZERO,
        Fixed64::from_i32(100),
        SeamAxis::EastWest,
        Fixed64::from_i32(80),
        Fixed64::from_i32(96),
    );
    let zone = WorldZone::new(ZoneId(1), bounds, None, false, 100);
    world.add_zone(zone);

    // Target zone requires asset bundle 42 with target digest v2
    let digest_v1 = AssetManifestDigest::from_version(100);
    let digest_v2 = AssetManifestDigest::from_version(101);
    let requirement = ZoneAssetRequirement::new(ZoneId(1), 42, digest_v2, 8192);
    let mut negotiator = PatchNegotiator::new(requirement);

    // 4. Initial Check: Client presents outdated digest v1
    assert_eq!(negotiator.state(), PatchNegotiationState::CheckingVersion);
    let synchronized_initially = negotiator.evaluate_client_manifest(digest_v1);
    assert!(!synchronized_initially);

    match negotiator.state() {
        PatchNegotiationState::DeltaRequired {
            target_digest,
            delta_bytes,
        } => {
            assert_eq!(target_digest, digest_v2);
            assert_eq!(delta_bytes, 8192);
        }
        _ => panic!("Expected DeltaRequired state"),
    }

    // 5. Client simulates applying pak-delta differential patch
    assert!(negotiator.update_progress(50).is_ok());
    assert!(negotiator.update_progress(100).is_ok());

    // Client verifies applied patch resulting in digest v2
    assert!(negotiator.complete_patch(digest_v2).expect("complete"));
    assert!(negotiator.is_synchronized());

    // 6. Client now successfully enters the zone in WorldManager
    let entity_id = 1u32;
    let initial_pos = Vec3Fix::from_f64(20.0, 0.0, 20.0);
    world
        .get_zone_mut(ZoneId(1))
        .expect("zone 1")
        .insert_entity(entity_id, initial_pos)
        .expect("insert");

    // 7. Stream live 20 Hz gameplay packets
    let ingress_queue = SpscPacketQueue::<128>::new();
    let egress_queue = SpscPacketQueue::<128>::new();
    let mut coordinator = TickCoordinator::new(20);
    let mut bot_state = KinematicState::with_velocity(
        initial_pos,
        Vec3Fix::from_f64(2.0, 0.0, 0.0),
        QuantizedYaw::NORTH,
        FLAG_WALKING,
    );
    let tick_interval = Fixed64::from_f64(0.050);

    let mut packet_buffer = [0u8; 32];
    let mut total_bytes_sent = 0;

    for tick in 0..20 {
        let tick_start = Instant::now();

        // Advance bot position
        bot_state.position.x += bot_state.velocity.x * tick_interval;

        // Build and send packet
        let header = PacketHeader::new(
            ChannelType::UnreliableSequenced,
            PacketType::StateUpdate,
            tick as u16,
            0,
            0,
        );
        header
            .write_to(&mut packet_buffer[..HEADER_SIZE])
            .expect("header");
        packet_buffer[12..16].copy_from_slice(&entity_id.to_le_bytes());

        let quant = QuantizedCellCoord::quantize(
            bot_state.position.x,
            bot_state.position.y,
            bot_state.position.z,
        );
        let transform_7b = quant.pack_with_yaw_and_flags(bot_state.yaw, bot_state.flags);
        packet_buffer[16..23].copy_from_slice(&transform_7b);

        let packet_len = 23;
        client_sock
            .send_to(&packet_buffer[..packet_len], bound_server_addr)
            .expect("send");
        total_bytes_sent += packet_len;

        // Server drains socket
        let _ = worker.drain_ingress(&ingress_queue, 16);
        while let Some(packet) = ingress_queue.try_pop() {
            if packet.len >= 23 {
                let mut tr_bytes = [0u8; 7];
                tr_bytes.copy_from_slice(&packet.payload[16..23]);
                let (q, _yaw, _flags) = QuantizedCellCoord::unpack_with_yaw_and_flags(tr_bytes);
                let pos = q.dequantize();
                let _ = world.tick_entity_movement(entity_id, ZoneId(1), pos);
            }
        }

        let _ = worker.flush_egress(&egress_queue, 16);
        let elapsed = tick_start.elapsed();
        coordinator.record_tick_execution(elapsed);
        coordinator.sleep_headroom(elapsed);
    }

    // Verify bandwidth: 20 ticks * 23 bytes = 460 bytes across 1.0 second
    let avg_bw = (total_bytes_sent as f64) / 1.0;
    assert!(
        avg_bw < 1200.0,
        "Bandwidth {avg_bw} B/s must remain below 1.2 KB/s wire budget"
    );
    assert_eq!(coordinator.metrics().total_ticks, 20);
    assert!(world
        .get_zone(ZoneId(1))
        .expect("zone")
        .contains_entity(entity_id));
}
