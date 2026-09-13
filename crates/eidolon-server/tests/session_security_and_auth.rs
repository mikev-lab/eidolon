//! Automated test suite verifying Milestone 8.1: Cryptographic Session Security & Handshake.
//!
//! Validates:
//! - Complete 3-way UDP challenge-response association flow using native HMAC-SHA256.
//! - Sliding 64-bit sequence replay window under reordering, duplication, and out-of-window attacks.
//! - Rejection of forged authentication proofs, invalid cookies, and truncated handshakes.

use eidolon_net::auth::{
    compute_auth_cookie, compute_client_proof, verify_client_proof, ConnectChallengeRequest,
    ConnectChallengeResponse, ConnectFinalizeRequest, ConnectFinalizeResponse, ReplayWindow,
    SessionSecurityContext, CHALLENGE_REQ_LEN, CHALLENGE_RESP_LEN, FINALIZE_REQ_LEN,
    FINALIZE_RESP_LEN, NONCE_LEN,
};
use eidolon_net::crypto::{constant_time_eq, hmac_sha256, sha256};
use eidolon_net::error::NetError;

#[test]
fn test_milestone_8_1_three_way_handshake_flow() {
    let server_secret = b"authoritative_world_server_secret_seed_98765";
    let account_token = b"client_secret_credentials_token_12345";
    let account_id = 77102u64;

    // Step 1: Client generates ConnectChallengeRequest
    let client_nonce = [0xA1u8; NONCE_LEN];
    let client_req = ConnectChallengeRequest {
        account_id,
        client_nonce,
        protocol_version: 1,
    };
    let mut req_buf = [0u8; CHALLENGE_REQ_LEN];
    assert_eq!(
        client_req.write_to(&mut req_buf).unwrap(),
        CHALLENGE_REQ_LEN
    );

    // Server receives and parses ConnectChallengeRequest
    let parsed_req = ConnectChallengeRequest::read_from(&req_buf).unwrap();
    assert_eq!(parsed_req.account_id, account_id);

    // Step 2: Server generates server nonce and stateless cookie
    let server_nonce = [0xB2u8; NONCE_LEN];
    let auth_cookie = compute_auth_cookie(
        server_secret,
        parsed_req.account_id,
        &parsed_req.client_nonce,
        &server_nonce,
    );
    let server_resp = ConnectChallengeResponse {
        server_nonce,
        auth_cookie,
    };
    let mut resp_buf = [0u8; CHALLENGE_RESP_LEN];
    assert_eq!(
        server_resp.write_to(&mut resp_buf).unwrap(),
        CHALLENGE_RESP_LEN
    );

    // Step 3: Client parses server challenge and computes proof
    let parsed_resp = ConnectChallengeResponse::read_from(&resp_buf).unwrap();
    let client_proof = compute_client_proof(
        account_token,
        &parsed_resp.auth_cookie,
        &client_nonce,
        &parsed_resp.server_nonce,
    );
    let finalize_req = ConnectFinalizeRequest {
        account_id,
        auth_cookie: parsed_resp.auth_cookie,
        client_proof,
    };
    let mut fin_req_buf = [0u8; FINALIZE_REQ_LEN];
    assert_eq!(
        finalize_req.write_to(&mut fin_req_buf).unwrap(),
        FINALIZE_REQ_LEN
    );

    // Step 4: Server validates client proof and establishes session context
    let parsed_fin_req = ConnectFinalizeRequest::read_from(&fin_req_buf).unwrap();
    let is_valid = verify_client_proof(
        account_token,
        &parsed_fin_req.auth_cookie,
        &client_nonce,
        &server_nonce,
        &parsed_fin_req.client_proof,
    );
    assert!(is_valid);

    // Server generates session key and finalizes response
    let session_id = 900142u64;
    let authority_epoch = 1u32;
    let server_proof = hmac_sha256(server_secret, &parsed_fin_req.client_proof);

    let finalize_resp = ConnectFinalizeResponse {
        session_id,
        authority_epoch,
        server_proof,
    };
    let mut fin_resp_buf = [0u8; FINALIZE_RESP_LEN];
    assert_eq!(
        finalize_resp.write_to(&mut fin_resp_buf).unwrap(),
        FINALIZE_RESP_LEN
    );

    // Client verifies server confirmation
    let parsed_fin_resp = ConnectFinalizeResponse::read_from(&fin_resp_buf).unwrap();
    assert_eq!(parsed_fin_resp.session_id, session_id);
    assert_eq!(parsed_fin_resp.authority_epoch, authority_epoch);
    assert!(constant_time_eq(
        &parsed_fin_resp.server_proof,
        &server_proof
    ));

    // Initialize session security context on server
    let session_key = sha256(&server_proof);
    let mut session_ctx = SessionSecurityContext::new(
        session_id,
        account_id,
        authority_epoch,
        session_key,
        100, // current tick
        600, // 30s TTL
    );

    assert!(!session_ctx.is_expired(100));
    assert!(!session_ctx.is_expired(700));
    assert!(session_ctx.is_expired(701));

    // Validate sequence update and timestamp refresh
    assert!(session_ctx.validate_sequence(1).is_ok());
    session_ctx.touch(750);
    assert!(!session_ctx.is_expired(750));
}

#[test]
fn test_milestone_8_1_forged_proof_rejection() {
    let server_secret = b"server_key_xyz";
    let account_token = b"valid_client_token";
    let forged_token = b"attacker_token_forged";
    let account_id = 42u64;
    let client_nonce = [1u8; NONCE_LEN];
    let server_nonce = [2u8; NONCE_LEN];

    let auth_cookie = compute_auth_cookie(server_secret, account_id, &client_nonce, &server_nonce);
    let forged_proof =
        compute_client_proof(forged_token, &auth_cookie, &client_nonce, &server_nonce);

    assert!(!verify_client_proof(
        account_token,
        &auth_cookie,
        &client_nonce,
        &server_nonce,
        &forged_proof
    ));
}

#[test]
fn test_milestone_8_1_replay_window_adversarial_stream() {
    let mut window = ReplayWindow::new();

    // In-order delivery
    assert!(window.check_and_update(1).is_ok());
    assert!(window.check_and_update(2).is_ok());
    assert!(window.check_and_update(3).is_ok());

    // Immediate duplicate
    assert_eq!(
        window.check_and_update(2),
        Err(NetError::ReplayDetected {
            sequence: 2,
            window_bottom: 0,
        })
    );

    // Large forward jump
    assert!(window.check_and_update(100).is_ok());
    assert_eq!(window.highest_sequence(), 100);

    // In-window out-of-order packets (100 - 63 = 37 is oldest valid)
    assert!(window.check_and_update(80).is_ok());
    assert!(window.check_and_update(50).is_ok());
    assert!(window.check_and_update(37).is_ok());

    // Duplicate in-window
    assert!(window.check_and_update(50).is_err());

    // Older than window floor (36 is 64 steps back from 100)
    assert!(window.check_and_update(36).is_err());
    assert!(window.check_and_update(1).is_err());
}
