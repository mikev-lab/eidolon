//! Integration test exercising the universal C ABI layer for game engine integration.

#![allow(unsafe_code)]

use std::ffi::{c_void, CString};
use std::net::UdpSocket;
use std::time::Duration;

use eidolon::*;
use eidolon_net::auth::{
    compute_auth_cookie, verify_client_proof, ConnectChallengeRequest, ConnectChallengeResponse,
    ConnectFinalizeRequest, ConnectFinalizeResponse, CHALLENGE_REQ_LEN, FINALIZE_REQ_LEN,
    NONCE_LEN,
};
use eidolon_net::packet::PacketHeader;
use eidolon_net::protocol::{ChannelType, PacketType, HEADER_SIZE};

// User data accumulator for C callback
struct TestCallbackData {
    events_received: Vec<EidolonEvent>,
}

unsafe extern "C" fn test_c_callback(event: *const EidolonEvent, user_data: *mut c_void) {
    if !event.is_null() && !user_data.is_null() {
        let data = &mut *(user_data as *mut TestCallbackData);
        data.events_received.push(*event);
    }
}

#[test]
fn test_c_abi_client_lifecycle_and_events() {
    // 1. Test handle creation and null pointer guards
    let handle = eidolon_client_create();
    assert!(!handle.is_null(), "Handle must not be null");

    assert_eq!(eidolon_client_is_connected(handle), 0);
    assert_eq!(
        eidolon_client_connect(std::ptr::null_mut(), std::ptr::null(), 7777, 1, 0, 0),
        EIDOLON_ERR_NULL_PTR
    );

    // 2. Set up loopback UDP server mock
    let server_socket = UdpSocket::bind("127.0.0.1:0").expect("Bind server socket");
    server_socket
        .set_nonblocking(true)
        .expect("Server non-blocking");
    let server_addr = server_socket.local_addr().expect("Server local addr");
    let server_port = server_addr.port();

    let account_id = 88888u64;
    let ticket_high = 0x1122334455667788u64;
    let ticket_low = 0x99AABBCCDDEEFF00u64;

    let mut ticket_bytes = [0u8; 16];
    ticket_bytes[..8].copy_from_slice(&ticket_high.to_be_bytes());
    ticket_bytes[8..16].copy_from_slice(&ticket_low.to_be_bytes());

    let host_cstr = CString::new("127.0.0.1").unwrap();

    // 3. Initiate connection via C ABI
    let conn_res = eidolon_client_connect(
        handle,
        host_cstr.as_ptr(),
        server_port,
        account_id,
        ticket_high,
        ticket_low,
    );
    assert_eq!(conn_res, EIDOLON_OK);

    // 4. Server handles Challenge Request
    let mut server_buf = [0u8; 1024];
    let mut client_peer = None;
    let mut client_nonce = [0u8; NONCE_LEN];
    let server_nonce = [0x77; NONCE_LEN];
    let server_secret = b"test_secret_for_ffi_test_1234567";
    let mut auth_cookie = [0u8; 32];

    for _ in 0..100 {
        if let Ok((len, peer)) = server_socket.recv_from(&mut server_buf) {
            client_peer = Some(peer);
            let (hdr, hdr_len) = PacketHeader::read_from(&server_buf[..len]).expect("Parse header");
            let payload = &server_buf[hdr_len..len];
            if payload.len() >= CHALLENGE_REQ_LEN {
                let challenge_req =
                    ConnectChallengeRequest::read_from(payload).expect("Challenge req");
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
                wire_resp[resp_hdr_len..resp_hdr_len + resp_len]
                    .copy_from_slice(&out_resp[..resp_len]);

                server_socket
                    .send_to(&wire_resp[..resp_hdr_len + resp_len], peer)
                    .expect("Send challenge resp");
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    let client_addr = client_peer.expect("Server must receive client packet");

    // 5. Client polls events, dispatches finalize request
    let mut cb_data = TestCallbackData {
        events_received: Vec::new(),
    };
    for _ in 0..100 {
        eidolon_client_poll_events(
            handle,
            Some(test_c_callback),
            &mut cb_data as *mut _ as *mut c_void,
        );
        std::thread::sleep(Duration::from_millis(1));
    }

    // 6. Server handles Finalize Request and sends Finalize Response
    for _ in 0..100 {
        if let Ok((len, _)) = server_socket.recv_from(&mut server_buf) {
            let (hdr, hdr_len) = PacketHeader::read_from(&server_buf[..len]).expect("Parse header");
            let payload = &server_buf[hdr_len..len];
            if payload.len() >= FINALIZE_REQ_LEN {
                let fin_req = ConnectFinalizeRequest::read_from(payload).expect("Parse finalize");
                let is_valid = verify_client_proof(
                    &ticket_bytes,
                    &auth_cookie,
                    &client_nonce,
                    &server_nonce,
                    &fin_req.client_proof,
                );
                assert!(is_valid);

                let fin_resp = ConnectFinalizeResponse {
                    session_id: 12345,
                    authority_epoch: 1,
                    server_proof: [0xAA; 32],
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
                    .send_to(&wire_fin[..fin_hdr_len + fin_resp_len], client_addr)
                    .expect("Send fin resp");
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    // 7. Client polls events and completes connection
    for _ in 0..100 {
        eidolon_client_poll_events(
            handle,
            Some(test_c_callback),
            &mut cb_data as *mut _ as *mut c_void,
        );
        if eidolon_client_is_connected(handle) == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    assert_eq!(eidolon_client_is_connected(handle), 1);
    assert!(cb_data
        .events_received
        .iter()
        .any(|e| e.event_type == EIDOLON_EVENT_CONNECTED && e.param1 == 12345));

    // 8. Server sends state update for entity 9001 at (64.0, 0.0, 64.0) with vx = 10 m/s
    let mut state_packet = [0u8; HEADER_SIZE + 22];
    let state_hdr = PacketHeader::new(
        ChannelType::UnreliableSequenced,
        PacketType::StateUpdate,
        3,
        12345,
        0,
    );
    let hdr_len = state_hdr
        .write_to(&mut state_packet[..HEADER_SIZE])
        .unwrap();

    let mut p_idx = hdr_len;
    state_packet[p_idx..p_idx + 4].copy_from_slice(&9001u32.to_be_bytes());
    p_idx += 4;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&1i16.to_be_bytes()); // cell_x
    p_idx += 2;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&1i16.to_be_bytes()); // cell_z
    p_idx += 2;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&0i16.to_be_bytes()); // cell_y
    p_idx += 2;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&0u16.to_be_bytes()); // q_x (at 64.0m)
    p_idx += 2;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&0u16.to_be_bytes()); // q_z (at 64.0m)
    p_idx += 2;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&0u16.to_be_bytes()); // q_y
    p_idx += 2;
    state_packet[p_idx] = 0; // yaw
    p_idx += 1;
    state_packet[p_idx] = 1; // flags
    p_idx += 1;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&10i16.to_be_bytes()); // vx = 10 m/s
    p_idx += 2;
    state_packet[p_idx..p_idx + 2].copy_from_slice(&0i16.to_be_bytes()); // vz = 0
    p_idx += 2;

    server_socket
        .send_to(&state_packet[..p_idx], client_addr)
        .unwrap();

    std::thread::sleep(Duration::from_millis(5));

    // 9. Poll events and verify entity 9001 spawned
    eidolon_client_poll_events(
        handle,
        Some(test_c_callback),
        &mut cb_data as *mut _ as *mut c_void,
    );

    assert_eq!(eidolon_client_get_visible_entity_count(handle), 1);
    let mut entity_ids = [0u32; 4];
    let written = eidolon_client_get_visible_entities(handle, entity_ids.as_mut_ptr(), 4);
    assert_eq!(written, 1);
    assert_eq!(entity_ids[0], 9001);

    // 10. Test entity extrapolation via C ABI (dt = 0.5s, should move +5.0m along X)
    let mut out_transform = EidolonTransform::default();
    let extrap_res = eidolon_client_extrapolate_entity(handle, 9001, 0.5, &mut out_transform);
    assert_eq!(extrap_res, EIDOLON_OK);
    assert!(
        (out_transform.x - 69.0).abs() < 1.0,
        "Position should advance from 64 to ~69m"
    );
    assert_eq!(out_transform.velocity_x, 10.0);

    // 11. Test intent and action dispatch via C ABI
    let intent_res = eidolon_client_send_intent(handle, 2.0, -1.0, 180.0, 1);
    assert_eq!(intent_res, EIDOLON_OK);

    let action_res = eidolon_client_send_action(handle, 1, 9001, 100);
    assert_eq!(action_res, EIDOLON_OK);

    // 12. Clean destruction
    eidolon_client_destroy(handle);
}
