//! Phase 26: WebAssembly (WASM) and WebTransport browser client compatibility tests.
//!
//! Validates that `WasmClient` seamlessly interoperates with authoritative server
//! state packet streams, accurately extracts 22-byte entity updates, extrapolates
//! entities at 60 FPS, populates `WebRenderEntity` render buffers, and produces
//! valid movement intent packets.

use eidolon_client::web::{WasmClient, WebRenderEntity, MAX_WEB_RENDER_ENTITIES};
use eidolon_client::{ClientConfig, ClientEvent};
use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw};
use eidolon_net::packet::PacketHeader;
use eidolon_net::protocol::{ChannelType, PacketType, HEADER_SIZE, PROTOCOL_VERSION};

#[test]
fn test_wasm_client_ingest_and_event_dispatch() {
    let mut client = WasmClient::new(ClientConfig::default());
    assert_eq!(client.world_view().count(), 0);

    // Build an authoritative state update packet with 2 entities
    let header = PacketHeader::new(
        ChannelType::UnreliableSequenced,
        PacketType::StateUpdate,
        1,
        0,
        0,
    );

    let mut packet = [0u8; 128];
    let hdr_len = header.write_to(&mut packet).expect("write header");

    let record_size = 22;
    let mut offset = hdr_len;

    // Entity 1: ID = 501, cell = (0, 0, 0), quant = (1000, 200, 1000), yaw = North (0), flags = 1, vx = 5, vz = 0
    let id1 = 501u32.to_be_bytes();
    packet[offset..offset + 4].copy_from_slice(&id1);
    packet[offset + 4..offset + 6].copy_from_slice(&0i16.to_be_bytes()); // cell_x
    packet[offset + 6..offset + 8].copy_from_slice(&0i16.to_be_bytes()); // cell_z
    packet[offset + 8..offset + 10].copy_from_slice(&0i16.to_be_bytes()); // cell_y
    packet[offset + 10..offset + 12].copy_from_slice(&1000u16.to_be_bytes()); // q_x
    packet[offset + 12..offset + 14].copy_from_slice(&1000u16.to_be_bytes()); // q_z
    packet[offset + 14..offset + 16].copy_from_slice(&200u16.to_be_bytes()); // q_y
    packet[offset + 16] = QuantizedYaw::NORTH.as_byte();
    packet[offset + 17] = 1; // flags
    packet[offset + 18..offset + 20].copy_from_slice(&5i16.to_be_bytes()); // vx
    packet[offset + 20..offset + 22].copy_from_slice(&0i16.to_be_bytes()); // vz
    offset += record_size;

    // Entity 2: ID = 502, cell = (1, 0, -1), quant = (500, 100, 500), yaw = East (64), flags = 2, vx = 0, vz = -3
    let id2 = 502u32.to_be_bytes();
    packet[offset..offset + 4].copy_from_slice(&id2);
    packet[offset + 4..offset + 6].copy_from_slice(&1i16.to_be_bytes()); // cell_x
    packet[offset + 6..offset + 8].copy_from_slice(&(-1i16).to_be_bytes()); // cell_z
    packet[offset + 8..offset + 10].copy_from_slice(&0i16.to_be_bytes()); // cell_y
    packet[offset + 10..offset + 12].copy_from_slice(&500u16.to_be_bytes()); // q_x
    packet[offset + 12..offset + 14].copy_from_slice(&500u16.to_be_bytes()); // q_z
    packet[offset + 14..offset + 16].copy_from_slice(&100u16.to_be_bytes()); // q_y
    packet[offset + 16] = QuantizedYaw::EAST.as_byte();
    packet[offset + 17] = 2; // flags
    packet[offset + 18..offset + 20].copy_from_slice(&0i16.to_be_bytes()); // vx
    packet[offset + 20..offset + 22].copy_from_slice(&(-3i16).to_be_bytes()); // vz
    offset += record_size;

    let parsed_count = client
        .ingest_datagram(&packet[..offset])
        .expect("ingest datagram");
    assert_eq!(parsed_count, 2);
    assert_eq!(client.world_view().count(), 2);

    // Verify events were pushed
    let mut event_count = 0;
    while let Some(event) = client.poll_event() {
        match event {
            ClientEvent::EntitySpawned {
                entity_id,
                entity_type,
                ..
            } => {
                assert!(entity_id == 501 || entity_id == 502);
                assert_eq!(entity_type, 0);
                event_count += 1;
            }
            other => panic!("Unexpected event: {:?}", other),
        }
    }
    assert_eq!(event_count, 2);
}

#[test]
fn test_wasm_render_entity_batch_population_and_extrapolation() {
    let mut client = WasmClient::new(ClientConfig::default());

    // Insert moving entity at origin: moving +10 m/s in X, +5 m/s in Z
    client.world_view_mut().upsert_entity(
        777,
        0,
        0,
        0,
        0,
        QuantizedCellCoord::new(0, 0, 0),
        QuantizedYaw::EAST,
        0x01,
        Vec3Fix {
            x: Fixed64::from_i32(10),
            y: Fixed64::ZERO,
            z: Fixed64::from_i32(5),
        },
    );

    // Populate render buffer with 0 delta time (initial frame)
    let mut render_batch = [WebRenderEntity::default(); MAX_WEB_RENDER_ENTITIES];
    let count0 = client.populate_render_entities(0.0, &mut render_batch);
    assert_eq!(count0, 1);
    assert_eq!(render_batch[0].entity_id, 777);
    assert_eq!(render_batch[0].x, 0.0);
    assert_eq!(render_batch[0].z, 0.0);

    // Simulate 60 FPS extrapolation for 1 second
    let count1 = client.populate_render_entities(1.0, &mut render_batch);
    assert_eq!(count1, 1);
    // At t = 1.0s, X should be approx 10.0m, Z should be approx 5.0m
    assert!((render_batch[0].x - 10.0).abs() < 0.1);
    assert!((render_batch[0].z - 5.0).abs() < 0.1);
}

#[test]
fn test_wasm_movement_intent_roundtrip() {
    let mut client = WasmClient::new(ClientConfig::default());
    let mut wire_buf = [0u8; 64];

    let bytes_written = client
        .encode_movement_intent(7.5, -3.25, 180.0, 0x03, &mut wire_buf)
        .expect("encode intent");

    assert_eq!(bytes_written, HEADER_SIZE + 13);

    // Verify packet header
    let (header, hdr_len) =
        PacketHeader::read_from(&wire_buf[..bytes_written]).expect("read header");
    assert_eq!(header.version, PROTOCOL_VERSION);
    assert_eq!(header.packet_type, PacketType::StateUpdate);
    assert_eq!(header.channel, ChannelType::UnreliableSequenced);
    assert_eq!(header.sequence, 1);

    // Verify payload
    let payload = &wire_buf[hdr_len..bytes_written];
    let vx = f32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let vz = f32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let yaw = f32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]);
    let flags = payload[12];

    assert_eq!(vx, 7.5);
    assert_eq!(vz, -3.25);
    assert_eq!(yaw, 180.0);
    assert_eq!(flags, 0x03);
}

#[test]
fn test_wasm_disconnect_packet_handling() {
    let mut client = WasmClient::new(ClientConfig::default());

    // Insert entity
    client.world_view_mut().upsert_entity(
        888,
        0,
        0,
        0,
        0,
        QuantizedCellCoord::new(0, 0, 0),
        QuantizedYaw::NORTH,
        0,
        Vec3Fix::ZERO,
    );
    assert_eq!(client.world_view().count(), 1);

    // Send disconnect packet
    let header = PacketHeader::new(
        ChannelType::ReliableOrdered,
        PacketType::Disconnect,
        99,
        0,
        0,
    );
    let mut buf = [0u8; 32];
    let len = header.write_to(&mut buf).expect("write header");

    client
        .ingest_datagram(&buf[..len])
        .expect("ingest disconnect");
    assert_eq!(client.world_view().count(), 0);

    let event = client.poll_event().expect("poll event");
    match event {
        ClientEvent::Disconnected { reason } => {
            assert_eq!(reason, "Server requested disconnect");
        }
        other => panic!("Expected Disconnected event, got: {:?}", other),
    }
}
