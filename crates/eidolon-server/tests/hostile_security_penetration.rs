//! Milestone 15.3 Integration Test Suite: Adversarial Internet-Facing Security Penetration Battery.
//!
//! Validates engine defenses against active hostile attackers:
//! 1. Cryptographic Handshake Tampering & Forgery (bit-flipping, forged HMAC proofs, nonce reflection).
//! 2. Session Hijacking, Token Stealing & Concurrent Login Eviction.
//! 3. Cross-Account Character Mutation & Authorization Fencing.
//! 4. Malformed Packet Parser Exploitation (integer overflows, truncated payloads, corrupt opcodes).
//! 5. Denial-of-Service (DoS) Unauthenticated Packet Storm Flooding.
//! 6. Constant-Time Verification Timing Side-Channel Immunity.

use std::hint::black_box;
use std::time::Instant;

use eidolon_core::identity::{
    AccountId, CharacterId, IdentityError, IdentityRegistry, SessionTicket,
};
use eidolon_net::auth::{
    compute_auth_cookie, compute_client_proof, verify_client_proof, NONCE_LEN,
};
use eidolon_net::bitstream::BitReader;
use eidolon_net::crypto::{constant_time_eq, hmac_sha256};
use eidolon_net::quota::{ConnectionMemoryAccountant, SocketRatePolicer, MAX_CONNECTION_MEMORY};
use eidolon_server::multi_process::{RealSocketPacket, CLUSTER_SOCKET_MAGIC, OP_MIGRATE};

#[test]
fn test_hostile_handshake_tampering_and_forgery() {
    let server_secret = b"eidolon_adversarial_master_secret_9999";
    let account_token = b"legitimate_account_credential_secret";
    let account_id = 88401u64;

    let client_nonce = [0x33u8; NONCE_LEN];
    let server_nonce = [0x77u8; NONCE_LEN];

    // Compute genuine cookie
    let genuine_cookie =
        compute_auth_cookie(server_secret, account_id, &client_nonce, &server_nonce);

    // Compute genuine proof
    let genuine_proof =
        compute_client_proof(account_token, &genuine_cookie, &client_nonce, &server_nonce);

    assert!(verify_client_proof(
        account_token,
        &genuine_cookie,
        &client_nonce,
        &server_nonce,
        &genuine_proof,
    ));

    // Attack 1: Single-bit flip attacks across all 32 bytes of the auth cookie
    for byte_idx in 0..32 {
        for bit_idx in 0..8 {
            let mut tampered_cookie = genuine_cookie;
            tampered_cookie[byte_idx] ^= 1 << bit_idx;

            let is_valid = verify_client_proof(
                account_token,
                &tampered_cookie,
                &client_nonce,
                &server_nonce,
                &genuine_proof,
            );
            assert!(
                !is_valid,
                "Tampered auth cookie (byte {byte_idx}, bit {bit_idx}) must be rejected"
            );
        }
    }

    // Attack 2: Forged client proofs (all-zeros, all-ones, inverted)
    let zero_proof = [0u8; 32];
    assert!(!verify_client_proof(
        account_token,
        &genuine_cookie,
        &client_nonce,
        &server_nonce,
        &zero_proof,
    ));

    let ones_proof = [0xFFu8; 32];
    assert!(!verify_client_proof(
        account_token,
        &genuine_cookie,
        &client_nonce,
        &server_nonce,
        &ones_proof,
    ));

    let mut inverted_proof = genuine_proof;
    for b in inverted_proof.iter_mut() {
        *b = !*b;
    }
    assert!(!verify_client_proof(
        account_token,
        &genuine_cookie,
        &client_nonce,
        &server_nonce,
        &inverted_proof,
    ));

    // Attack 3: Nonce reflection attack (attacker passes server nonce back as client proof)
    let mut reflected_proof = [0u8; 32];
    reflected_proof[0..NONCE_LEN].copy_from_slice(&server_nonce);
    assert!(!verify_client_proof(
        account_token,
        &genuine_cookie,
        &client_nonce,
        &server_nonce,
        &reflected_proof,
    ));
}

#[test]
fn test_hostile_session_hijacking_and_concurrent_login_fencing() {
    let mut registry: IdentityRegistry<128, 512> = IdentityRegistry::new();

    let account_alpha = AccountId(7001);
    let char_alpha = CharacterId(9001);

    registry
        .register_character(account_alpha, char_alpha)
        .expect("Register character");

    // Session 1 (Legitimate player logs in from primary device)
    let ticket_1 = SessionTicket::new([1u8; 16]);
    let gen_1 = registry
        .authenticate_session(account_alpha, ticket_1)
        .expect("Authenticate session 1");
    assert_eq!(gen_1, 1);

    let auth_1 = registry
        .select_character(&ticket_1, char_alpha)
        .expect("Select character session 1");
    assert_eq!(auth_1.generation, 1);

    // Legitimate actions succeed
    assert!(registry
        .validate_character_action(&auth_1, char_alpha)
        .is_ok());

    // Attack 1: Attacker attempts to guess/forge an unregistered session ticket
    let forged_ticket = SessionTicket::new([0xDE; 16]);
    let forged_select = registry.select_character(&forged_ticket, char_alpha);
    assert_eq!(forged_select, Err(IdentityError::SessionRevoked));

    // Attack 2: Attacker initiates concurrent login (or legitimate user logs in from secondary device)
    let ticket_2 = SessionTicket::new([2u8; 16]);
    let gen_2 = registry
        .authenticate_session(account_alpha, ticket_2)
        .expect("Authenticate session 2");
    assert_eq!(
        gen_2, 2,
        "Monotonic generation must increment on concurrent login"
    );

    let auth_2 = registry
        .select_character(&ticket_2, char_alpha)
        .expect("Select character session 2");
    assert_eq!(auth_2.generation, 2);

    // Session 2 is now active
    assert!(registry
        .validate_character_action(&auth_2, char_alpha)
        .is_ok());

    // Attack 3: Prior session attempts to send in-flight or delayed mutation commands
    let stale_action = registry.validate_character_action(&auth_1, char_alpha);
    assert!(
        matches!(
            stale_action,
            Err(IdentityError::SessionRevoked) | Err(IdentityError::StaleSessionGeneration { .. })
        ),
        "Prior session generation must be immediately fenced upon concurrent login"
    );
}

#[test]
fn test_hostile_cross_account_character_mutation_exploit() {
    let mut registry: IdentityRegistry<128, 512> = IdentityRegistry::new();

    let account_victim = AccountId(1001);
    let char_victim = CharacterId(5001);

    let account_attacker = AccountId(2002);
    let char_attacker = CharacterId(6002);

    registry
        .register_character(account_victim, char_victim)
        .expect("Register victim character");
    registry
        .register_character(account_attacker, char_attacker)
        .expect("Register attacker character");

    let attacker_ticket = SessionTicket::new([0xAA; 16]);
    registry
        .authenticate_session(account_attacker, attacker_ticket)
        .expect("Authenticate attacker");

    // Attack 1: Attacker tries to select victim's high-value character during session initialization
    let select_exploit = registry.select_character(&attacker_ticket, char_victim);
    assert_eq!(
        select_exploit,
        Err(IdentityError::CrossAccountCharacterMismatch {
            character_id: char_victim,
            owner_account: account_victim,
            caller_account: account_attacker,
        })
    );

    // Attacker selects their own character
    let attacker_auth = registry
        .select_character(&attacker_ticket, char_attacker)
        .expect("Select attacker char");

    // Attack 2: Attacker crafts a command packet claiming their auth token but targeting victim character
    let mutation_exploit = registry.validate_character_action(&attacker_auth, char_victim);
    assert!(
        matches!(
            mutation_exploit,
            Err(IdentityError::CrossAccountCharacterMismatch {
                character_id,
                ..
            }) if character_id == char_victim
        ),
        "Cross-account character action must be strictly rejected"
    );
}

#[test]
fn test_hostile_packet_parser_fuzzing_and_integer_overflows() {
    // 1. Integer overflow in payload length claims (claiming 65535 bytes with truncated datagram)
    let mut overflow_packet = vec![0u8; 32];
    overflow_packet[0..4].copy_from_slice(&CLUSTER_SOCKET_MAGIC.to_be_bytes());
    overflow_packet[4] = OP_MIGRATE;
    overflow_packet[5..13].copy_from_slice(&9001u64.to_be_bytes()); // identifier
    overflow_packet[13..21].copy_from_slice(&1u64.to_be_bytes()); // sequence
    overflow_packet[21..23].copy_from_slice(&0xFFFFu16.to_be_bytes()); // Claims 65,535 bytes!

    let decoded = RealSocketPacket::decode(&overflow_packet);
    assert!(
        decoded.is_err(),
        "Parser must reject oversized payload length claims without buffer overrun"
    );

    // 2. Truncated header (packet shorter than 23-byte minimum)
    let truncated = [0u8; 12];
    assert!(RealSocketPacket::decode(&truncated).is_err());

    // 3. Invalid magic numbers
    let mut bad_magic = overflow_packet.clone();
    bad_magic[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
    assert!(RealSocketPacket::decode(&bad_magic).is_err());

    // 4. BitReader out-of-bounds bit reading
    let raw_bytes = [0xAA, 0xBB, 0xCC, 0xDD];
    let mut reader = BitReader::new(&raw_bytes);
    assert!(reader.read_bits(32).is_ok());
    // Attempting to read 1 more bit past buffer end returns typed error, never panic
    assert!(reader.read_bits(1).is_err());
}

#[test]
fn test_hostile_unauthenticated_dos_packet_storm() {
    // Rate policer: 40 pps burst, 32 KB/sec ceiling
    let mut rate_policer = SocketRatePolicer::new(40, 32768);
    let mut accountant = ConnectionMemoryAccountant::new(MAX_CONNECTION_MEMORY);

    let mut accepted = 0;
    let mut dropped = 0;

    // Attacker floods 50,000 UDP datagrams in a single burst
    for _ in 0..50_000 {
        if rate_policer.check_ingress(128, 1).is_ok() {
            accepted += 1;
        } else {
            dropped += 1;
        }
    }

    assert_eq!(
        accepted, 40,
        "Policer must enforce strict burst limit of 40 packets"
    );
    assert_eq!(
        dropped, 49_960,
        "Policer must drop all 49,960 excess flood packets"
    );

    // Verify memory accountant prevents unbounded heap growth under hostile flood
    let mut memory_allocated = 0;
    let mut allocation_rejected = 0;
    for _ in 0..10 {
        if accountant.track_allocation(10 * 1024).is_ok() {
            memory_allocated += 1;
        } else {
            allocation_rejected += 1;
        }
    }

    assert_eq!(
        memory_allocated, 6,
        "Must strictly cap at 60 KB (under 64 KB ceiling)"
    );
    assert_eq!(
        allocation_rejected, 4,
        "Must reject all allocations exceeding 64 KB ceiling"
    );
}

#[test]
fn test_hostile_constant_time_timing_side_channel_barrier() {
    let secret = b"super_secret_audit_key_for_side_channels";
    let data = b"payload_message_for_constant_time_verification";
    let valid_tag = hmac_sha256(secret, data);

    // Tag with mismatch in first byte
    let mut mismatch_first = valid_tag;
    mismatch_first[0] ^= 0x01;

    // Tag with mismatch in last byte
    let mut mismatch_last = valid_tag;
    mismatch_last[31] ^= 0x01;

    // Verify constant-time comparison rejects both
    assert!(!constant_time_eq(&valid_tag, &mismatch_first));
    assert!(!constant_time_eq(&valid_tag, &mismatch_last));
    assert!(constant_time_eq(&valid_tag, &valid_tag));

    // Warmup cycle to stabilize instruction cache and branch target buffers
    let warmup = 5_000u64;
    for _ in 0..warmup {
        black_box(constant_time_eq(
            black_box(&valid_tag),
            black_box(&mismatch_first),
        ));
        black_box(constant_time_eq(
            black_box(&valid_tag),
            black_box(&mismatch_last),
        ));
    }

    // Interleaved batched measurement to neutralize OS scheduling jitter and frequency scaling
    let rounds = 100usize;
    let batch = 500usize;
    let mut elapsed_first_nanos = 0u128;
    let mut elapsed_last_nanos = 0u128;

    for _ in 0..rounds {
        let s1 = Instant::now();
        for _ in 0..batch {
            black_box(constant_time_eq(
                black_box(&valid_tag),
                black_box(&mismatch_first),
            ));
        }
        elapsed_first_nanos += s1.elapsed().as_nanos();

        let s2 = Instant::now();
        for _ in 0..batch {
            black_box(constant_time_eq(
                black_box(&valid_tag),
                black_box(&mismatch_last),
            ));
        }
        elapsed_last_nanos += s2.elapsed().as_nanos();
    }

    let total_ops = (rounds * batch) as f64;
    let diff_nanos = (elapsed_first_nanos as i128 - elapsed_last_nanos as i128).abs();
    let per_op_diff_nanos = diff_nanos as f64 / total_ops;

    println!(
        "Constant-time timing difference per op: {:.4} ns (mismatch at byte 0 vs byte 31)",
        per_op_diff_nanos
    );

    // In debug mode, allow for unoptimized function call frames and OS jitter; in release mode, verify tight invariance
    let max_allowed_diff = if cfg!(debug_assertions) { 100.0 } else { 5.0 };
    assert!(
        per_op_diff_nanos < max_allowed_diff,
        "Early-exit timing discrepancy detected: {:.4} ns/op (threshold: {:.1} ns/op)",
        per_op_diff_nanos,
        max_allowed_diff
    );
}
