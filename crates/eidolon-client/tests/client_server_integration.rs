//! Integration test validating native Rust EidolonClient networking, handshake, and extrapolation.

use std::net::UdpSocket;
use std::time::Duration;

use eidolon_client::{ClientConfig, ClientEvent, EidolonClient};
use eidolon_core::identity::{AccountId, SessionTicket};
use eidolon_net::auth::{
    compute_auth_cookie, verify_client_proof, ConnectChallengeRequest, ConnectChallengeResponse,
    ConnectFinalizeRequest, ConnectFinalizeResponse, CHALLENGE_REQ_LEN, FINALIZE_REQ_LEN,
    NONCE_LEN,
};
use eidolon_net::packet::PacketHeader;
use eidolon_net::protocol::{ChannelType, PacketType, HEADER_SIZE};

#[test]
fn test_client_server_handshake_and_extrapolation_flow() {
    // 1. Bind mock server UDP socket on loopback
    let server_socket = UdpSocket::bind("127.0.0.1:0").expect("Bind server socket");
    server_socket
        .set_nonblocking(true)
        .expect("Server non-blocking");
    let server_addr = server_socket.local_addr().expect("Server local addr");

    let server_secret = b"test_server_secret_key_12345678";
    let account_id = 4242u64;
    let ticket_bytes = [7u8; 16];
    let session_ticket = SessionTicket(ticket_bytes);

    // 2. Initialize and connect EidolonClient
    let config = ClientConfig::new(server_addr, AccountId(account_id), session_ticket)
        .expect("Create config");
    let mut client = EidolonClient::new(config).expect("Create client");

    // Initiate challenge handshake
    client.connect().expect("Client connect");

    // 3. Server handles Challenge Request
    let mut server_buf = [0u8; 1024];
    let mut client_addr = None;
    let mut client_nonce = [0u8; NONCE_LEN];
    let server_nonce = [0x5A; NONCE_LEN];
    let mut auth_cookie = [0u8; 32];

    for _ in 0..100 {
        if let Ok((len, peer)) = server_socket.recv_from(&mut server_buf) {
            client_addr = Some(peer);
            let (hdr, hdr_len) = PacketHeader::read_from(&server_buf[..len]).expect("Parse header");
            assert_eq!(hdr.packet_type, PacketType::ReliableMessage);

            let payload = &server_buf[hdr_len..len];
            assert!(payload.len() >= CHALLENGE_REQ_LEN);

            let challenge_req = ConnectChallengeRequest::read_from(payload).expect("Challenge req");
            assert_eq!(challenge_req.account_id, account_id);
            client_nonce = challenge_req.client_nonce;

            auth_cookie =
                compute_auth_cookie(server_secret, account_id, &client_nonce, &server_nonce);

            let resp = ConnectChallengeResponse {
                server_nonce,
                auth_cookie,
            };

            let mut out_resp = [0u8; 64];
            let resp_len = resp.write_to(&mut out_resp).expect("Write resp");

            let resp_hdr = PacketHeader::new(
                ChannelType::ReliableOrdered,
                PacketType::ReliableMessage,
                1,
                hdr.sequence,
                0,
            );

            let mut wire_resp = [0u8; 128];
            let resp_hdr_len = resp_hdr.write_to(&mut wire_resp).expect("Write hdr");
            wire_resp[resp_hdr_len..resp_hdr_len + resp_len].copy_from_slice(&out_resp[..resp_len]);

            server_socket
                .send_to(&wire_resp[..resp_hdr_len + resp_len], peer)
                .expect("Send challenge resp");
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    let client_peer = client_addr.expect("Server must receive client packet");

    // 4. Client polls events, receives challenge response, emits finalize request
    for _ in 0..100 {
        let _ = client.poll_events();
        if client.session_id().is_some() || client.poll_events().is_ok() {
            // Check if finalize sent
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    // 5. Server handles Finalize Request and sends Finalize Response
    for _ in 0..100 {
        if let Ok((len, _)) = server_socket.recv_from(&mut server_buf) {
            let (hdr, hdr_len) = PacketHeader::read_from(&server_buf[..len]).expect("Parse header");
            let payload = &server_buf[hdr_len..len];
            if payload.len() >= FINALIZE_REQ_LEN {
                let fin_req = ConnectFinalizeRequest::read_from(payload).expect("Parse finalize");
                assert_eq!(fin_req.account_id, account_id);
                assert_eq!(fin_req.auth_cookie, auth_cookie);

                let is_valid = verify_client_proof(
                    &ticket_bytes,
                    &auth_cookie,
                    &client_nonce,
                    &server_nonce,
                    &fin_req.client_proof,
                );
                assert!(is_valid, "Client proof must be cryptographically valid");

                let fin_resp = ConnectFinalizeResponse {
                    session_id: 99999,
                    authority_epoch: 1,
                    server_proof: [0xEE; 32],
                };

                let mut out_fin_resp = [0u8; 64];
                let fin_resp_len = fin_resp
                    .write_to(&mut out_fin_resp)
                    .expect("Write fin resp");

                let resp_hdr = PacketHeader::new(
                    ChannelType::ReliableOrdered,
                    PacketType::ReliableMessage,
                    2,
                    hdr.sequence,
                    0,
                );

                let mut wire_fin = [0u8; 128];
                let fin_hdr_len = resp_hdr.write_to(&mut wire_fin).expect("Write hdr");
                wire_fin[fin_hdr_len..fin_hdr_len + fin_resp_len]
                    .copy_from_slice(&out_fin_resp[..fin_resp_len]);

                server_socket
                    .send_to(&wire_fin[..fin_hdr_len + fin_resp_len], client_peer)
                    .expect("Send fin resp");
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    // 6. Client polls events and completes connection
    let mut events = Vec::new();
    for _ in 0..100 {
        if let Ok(evts) = client.poll_events() {
            events.extend(evts);
        }
        if client.is_connected() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    assert!(client.is_connected(), "Client must be in connected state");
    assert_eq!(client.session_id(), Some(99999));
    assert_eq!(client.authority_epoch(), Some(1));

    assert!(events.iter().any(|e| matches!(
        e,
        ClientEvent::Connected {
            server_version: _,
            session_id: 99999
        }
    )));

    // 7. Server streams entity transform update for Entity 7001 (Goblin Scout)
    // Moving at vx = 4 m/s, vz = 0 m/s from (X=100.0, Z=100.0)
    let entity_id = 7001u32;
    let cell_x = 1i16; // 64m..128m
    let cell_z = 1i16;
    let cell_y = 0i16;
    let q_x = 36864u16; // (100 - 64) / 64 * 65535 = 36864
    let q_z = 36864u16;
    let q_y = 0u16;
    let yaw_byte = 64u8; // 90 degrees (East)
    let flags = 1u8; // Walking
    let vx = 4i16; // 4 m/s
    let vz = 0i16;

    let mut state_packet = [0u8; HEADER_SIZE + 22];
    let state_hdr = PacketHeader::new(
        ChannelType::UnreliableSequenced,
        PacketType::StateUpdate,
        3,
        client.session_id().unwrap() as u16,
        0,
    );
    let hdr_len = state_hdr
        .write_to(&mut state_packet[..HEADER_SIZE])
        .expect("Write state hdr");

    let mut p_idx = hdr_len;
    state_packet[p_idx..p_idx + 4].copy_from_slice(&entity_id.to_be_bytes());
    p_idx += 4;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&cell_x.to_be_bytes());
    p_idx += 2;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&cell_z.to_be_bytes());
    p_idx += 2;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&cell_y.to_be_bytes());
    p_idx += 2;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&q_x.to_be_bytes());
    p_idx += 2;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&q_z.to_be_bytes());
    p_idx += 2;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&q_y.to_be_bytes());
    p_idx += 2;
    state_packet[p_idx] = yaw_byte;
    p_idx += 1;
    state_packet[p_idx] = flags;
    p_idx += 1;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&vx.to_be_bytes());
    p_idx += 2;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&vz.to_be_bytes());
    p_idx += 2;

    server_socket
        .send_to(&state_packet[..p_idx], client_peer)
        .expect("Send state update");

    std::thread::sleep(Duration::from_millis(5));

    // 8. Client polls events and processes EntitySpawned
    let spawn_events = client.poll_events().expect("Poll state events");
    assert_eq!(client.visible_entity_count(), 1);
    assert_eq!(client.get_visible_entities(), vec![7001]);

    let spawned = spawn_events
        .iter()
        .find(|e| {
            matches!(
                e,
                ClientEvent::EntitySpawned {
                    entity_id: 7001,
                    ..
                }
            )
        })
        .expect("Entity 7001 must be spawned");

    if let ClientEvent::EntitySpawned {
        x, y, z, yaw_deg, ..
    } = spawned
    {
        assert!((x - 100.0).abs() < 1.0);
        assert_eq!(*y, 0.0);
        assert!((z - 100.0).abs() < 1.0);
        assert!((yaw_deg - 90.0).abs() < 2.0);
    }

    // 9. Dead Reckoning Extrapolation Test at 60 FPS (dt = 0.5s)
    // Moving at 4.0 m/s, after 0.5s position should advance by 2.0m along X axis
    let extrapolated = client
        .extrapolate_entity(7001, 0.5)
        .expect("Extrapolate entity");

    assert!(
        (extrapolated.x - 102.0).abs() < 1.0,
        "Extrapolated position X ({}) should be approx 102.0 (100.0 + 4m/s * 0.5s)",
        extrapolated.x
    );
    assert!((extrapolated.z - 100.0).abs() < 1.0);
    assert_eq!(extrapolated.vx, 4.0);

    // 10. Client sends movement intent and combat action
    client
        .send_movement_intent(1.5, 0.0, 45.0, 1)
        .expect("Send intent");
    client.send_action(1, 7001, 50).expect("Send attack action");

    // Verify server receives action
    std::thread::sleep(Duration::from_millis(5));
    let mut actions_received = 0;
    while let Ok((len, _)) = server_socket.recv_from(&mut server_buf) {
        let (hdr, _) = PacketHeader::read_from(&server_buf[..len]).expect("Parse header");
        if hdr.packet_type == PacketType::ReliableMessage {
            actions_received += 1;
        }
    }
    assert!(
        actions_received >= 1,
        "Server must receive reliable gameplay action"
    );

    // 11. Disconnect test
    client.disconnect();
    assert!(!client.is_connected());
    assert_eq!(client.visible_entity_count(), 0);
}
