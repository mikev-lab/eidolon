//! Milestone 14.4 Integration Test Suite: Identity & Authorization Pipeline.
//!
//! Validates the 3-tier authorization hierarchy (Account -> Session -> Character),
//! proves concurrent login revocation via monotonic generation fencing, and verifies
//! cross-account mutation write protections.

use eidolon_core::identity::{
    AccountId, CharacterId, IdentityError, IdentityRegistry, SessionTicket,
};

#[test]
fn test_milestone_14_4_three_tier_authorization_happy_path() {
    let mut registry = IdentityRegistry::<32, 128>::new();

    let account = AccountId(5001);
    let char_knight = CharacterId(9001);
    let char_mage = CharacterId(9002);

    registry.register_character(account, char_knight).unwrap();
    registry.register_character(account, char_mage).unwrap();

    let ticket = SessionTicket([0x12; 16]);
    let generation = registry.authenticate_session(account, ticket).unwrap();
    assert_eq!(generation, 1);
    assert_eq!(registry.active_session_count(), 1);

    // Select character
    let auth = registry.select_character(&ticket, char_knight).unwrap();
    assert_eq!(auth.account_id, account);
    assert_eq!(auth.character_id, char_knight);
    assert_eq!(auth.generation, 1);

    // Authorize valid action
    assert!(registry
        .validate_character_action(&auth, char_knight)
        .is_ok());

    // Switch active character to Mage
    let auth_mage = registry.select_character(&ticket, char_mage).unwrap();
    assert_eq!(auth_mage.character_id, char_mage);
    assert!(registry
        .validate_character_action(&auth_mage, char_mage)
        .is_ok());
}

#[test]
fn test_milestone_14_4_concurrent_login_fencing_revokes_prior_session() {
    let mut registry = IdentityRegistry::<32, 128>::new();

    let player = AccountId(777);
    let character = CharacterId(101);
    registry.register_character(player, character).unwrap();

    // Session 1 logs in (Mobile device)
    let ticket_mobile = SessionTicket([0xAA; 16]);
    let gen_1 = registry
        .authenticate_session(player, ticket_mobile)
        .unwrap();
    assert_eq!(gen_1, 1);

    let auth_mobile = registry
        .select_character(&ticket_mobile, character)
        .unwrap();

    // Mobile sends command: succeeds
    assert!(registry
        .validate_character_action(&auth_mobile, character)
        .is_ok());

    // Session 2 logs in with same account (Desktop PC)
    let ticket_desktop = SessionTicket([0xBB; 16]);
    let gen_2 = registry
        .authenticate_session(player, ticket_desktop)
        .unwrap();
    assert_eq!(gen_2, 2);

    // Prior mobile session capability token is now completely fenced
    let mobile_err = registry.validate_character_action(&auth_mobile, character);
    assert!(
        matches!(
            mobile_err,
            Err(IdentityError::SessionRevoked) | Err(IdentityError::StaleSessionGeneration { .. })
        ),
        "Evicted mobile session must be rejected by generation fence"
    );

    // Desktop session selects character and proceeds
    let auth_desktop = registry
        .select_character(&ticket_desktop, character)
        .unwrap();
    assert_eq!(auth_desktop.generation, 2);
    assert!(registry
        .validate_character_action(&auth_desktop, character)
        .is_ok());
}

#[test]
fn test_milestone_14_4_cross_account_write_protection() {
    let mut registry = IdentityRegistry::<32, 128>::new();

    let alice_acc = AccountId(1001);
    let bob_acc = AccountId(2002);

    let alice_char = CharacterId(555);
    let bob_char = CharacterId(666);

    registry.register_character(alice_acc, alice_char).unwrap();
    registry.register_character(bob_acc, bob_char).unwrap();

    let alice_ticket = SessionTicket([0x11; 16]);
    let bob_ticket = SessionTicket([0x22; 16]);

    registry
        .authenticate_session(alice_acc, alice_ticket)
        .unwrap();
    registry.authenticate_session(bob_acc, bob_ticket).unwrap();

    // Alice attempts to select Bob's character: rejected with CrossAccountCharacterMismatch
    let alice_exploit = registry.select_character(&alice_ticket, bob_char);
    assert_eq!(
        alice_exploit,
        Err(IdentityError::CrossAccountCharacterMismatch {
            character_id: bob_char,
            owner_account: bob_acc,
            caller_account: alice_acc,
        })
    );

    // Alice creates valid auth for her own character
    let alice_auth = registry
        .select_character(&alice_ticket, alice_char)
        .unwrap();

    // Alice attempts to execute an action targeting Bob's character using her auth token
    let cross_action_err = registry.validate_character_action(&alice_auth, bob_char);
    assert!(
        cross_action_err.is_err(),
        "Must reject action targeting another player's character"
    );

    // Bob can legitimately select and act with his character
    let bob_auth = registry.select_character(&bob_ticket, bob_char).unwrap();
    assert!(registry
        .validate_character_action(&bob_auth, bob_char)
        .is_ok());
}

#[test]
fn test_milestone_14_4_clean_logout_and_revocation() {
    let mut registry = IdentityRegistry::<16, 64>::new();

    let acc = AccountId(3001);
    let character = CharacterId(4001);
    registry.register_character(acc, character).unwrap();

    let ticket = SessionTicket([0x99; 16]);
    registry.authenticate_session(acc, ticket).unwrap();
    let auth = registry.select_character(&ticket, character).unwrap();

    assert_eq!(registry.active_session_count(), 1);
    assert!(registry.validate_character_action(&auth, character).is_ok());

    // Explicit logout revocation
    registry.revoke_session(acc);
    assert_eq!(registry.active_session_count(), 0);

    // Action after logout must be rejected
    let err = registry.validate_character_action(&auth, character);
    assert_eq!(err, Err(IdentityError::SessionRevoked));
}
