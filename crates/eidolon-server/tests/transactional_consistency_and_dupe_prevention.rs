//! Milestone 9.2: Double-Spend Prevention, Multi-Session Fencing and Transactional Consistency.
//!
//! Verifies atomic transaction boundaries, generation-locked multi-session fencing,
//! and complete immunity against double-spend exploits and item duplication bugs.

use eidolon_world::transaction::{
    AccountState, TransactionManager, TransactionOp, LOCK_INVENTORY, LOCK_WALLET,
};
use eidolon_world::wal::{MockDurableStorage, WriteAheadJournal};
use eidolon_world::WorldError;

#[test]
fn test_milestone_9_2_adversarial_double_spend_100_attempts() {
    let mut manager = TransactionManager::new();

    // Account 1001 initialized with 1,000 gems
    let mut account = AccountState::new(1001);
    account.premium_currency = 1_000;
    manager.register_account(account);

    let mut journal = WriteAheadJournal::<2048>::new(Box::new(MockDurableStorage::new()));

    // Active session 777 acquires wallet lock
    let token = manager
        .acquire_lock(1001, LOCK_WALLET, 777, 10, 500, false)
        .expect("Lock acquisition must succeed");

    let mut successes = 0;
    let mut rejections = 0;

    // Simulate 100 concurrent/rapid attempts to spend 1,000 gems
    for tx_id in 1..=100 {
        let result = manager.execute_transaction(
            tx_id,
            TransactionOp::CurrencySpend {
                account_id: 1001,
                amount: 1_000,
                is_premium: true,
            },
            777,
            &token,
            15,
            &mut journal,
        );

        match result {
            Ok(_) => successes += 1,
            Err(WorldError::InsufficientBalance { .. }) => rejections += 1,
            Err(other) => panic!("Unexpected error: {other:?}"),
        }
    }

    // Invariant: Exactly 1 spend succeeded, 99 rejected with InsufficientBalance
    assert_eq!(
        successes, 1,
        "Exactly 1 transaction must succeed in double-spend attack"
    );
    assert_eq!(
        rejections, 99,
        "99 transactions must be rejected with InsufficientBalance"
    );

    // Final balance must be exactly 0
    let final_balance = manager.get_account(1001).unwrap().premium_currency;
    assert_eq!(final_balance, 0, "Account balance must be exactly 0");
}

#[test]
fn test_milestone_9_2_atomic_item_transfer_zero_duplication() {
    let mut manager = TransactionManager::new();

    // Account A owns 1 Divine Relic (Item 777)
    let mut account_a = AccountState::new(1001);
    account_a.add_item(777, 1).expect("Add item to A");
    manager.register_account(account_a);

    // Account B starts with 0 items
    let account_b = AccountState::new(2002);
    manager.register_account(account_b);

    let mut journal = WriteAheadJournal::<2048>::new(Box::new(MockDurableStorage::new()));

    let token_a = manager
        .acquire_lock(1001, LOCK_INVENTORY, 50, 1, 100, false)
        .unwrap();

    // Transaction 1: Transfer item 777 from A to B
    let lsn = manager
        .execute_transaction(
            1,
            TransactionOp::ItemTransfer {
                from_account: 1001,
                to_account: 2002,
                item_id: 777,
                quantity: 1,
            },
            50,
            &token_a,
            2,
            &mut journal,
        )
        .expect("First item transfer must succeed");
    assert_eq!(lsn, 1);

    // Assert A no longer has item 777
    let a_items = manager.get_account(1001).unwrap().inventory;
    assert!(a_items.iter().flatten().all(|i| i.item_id != 777));

    // Assert B now has exactly 1 of item 777
    let b_items = manager.get_account(2002).unwrap().inventory;
    let b_relic = b_items.iter().flatten().find(|i| i.item_id == 777);
    assert_eq!(b_relic.map(|i| i.quantity), Some(1));

    // Transaction 2: Adversarial attempt to transfer item 777 again from A to B
    let err = manager
        .execute_transaction(
            2,
            TransactionOp::ItemTransfer {
                from_account: 1001,
                to_account: 2002,
                item_id: 777,
                quantity: 1,
            },
            50,
            &token_a,
            3,
            &mut journal,
        )
        .expect_err("Duplicate transfer must be rejected");

    assert_eq!(err, WorldError::ItemNotFound(777));

    // Total item count across all accounts must remain exactly 1 (Zero Duplication Invariant)
    let total_relics = manager
        .get_account(1001)
        .unwrap()
        .inventory
        .iter()
        .flatten()
        .filter(|i| i.item_id == 777)
        .map(|i| i.quantity)
        .sum::<u32>()
        + manager
            .get_account(2002)
            .unwrap()
            .inventory
            .iter()
            .flatten()
            .filter(|i| i.item_id == 777)
            .map(|i| i.quantity)
            .sum::<u32>();
    assert_eq!(total_relics, 1, "Zero duplication invariant violated");
}

#[test]
fn test_milestone_9_2_stale_session_reconnect_fencing() {
    let mut manager = TransactionManager::new();
    let mut account = AccountState::new(3003);
    account.free_currency = 50_000;
    manager.register_account(account);

    let mut journal = WriteAheadJournal::<2048>::new(Box::new(MockDurableStorage::new()));

    // Old session 10 acquires lock
    let old_token = manager
        .acquire_lock(3003, LOCK_WALLET, 10, 100, 50, false)
        .unwrap();

    // Client reconnects on new session 20, forcing takeover and bumping generation
    let new_token = manager
        .acquire_lock(3003, LOCK_WALLET, 20, 110, 50, true)
        .unwrap();
    assert_eq!(new_token.generation, 2);

    // Old session attempts spend with stale token and session ID: must be rejected
    let stale_err = manager
        .execute_transaction(
            1,
            TransactionOp::CurrencySpend {
                account_id: 3003,
                amount: 10_000,
                is_premium: false,
            },
            10,
            &old_token,
            115,
            &mut journal,
        )
        .expect_err("Old session mutation must be rejected");

    match stale_err {
        WorldError::StaleSessionMutation { expected, actual } => {
            assert_eq!(expected, 20);
            assert_eq!(actual, 10);
        }
        other => panic!("Expected StaleSessionMutation, got {other:?}"),
    }

    // New session performs legitimate spend: succeeds
    let lsn = manager
        .execute_transaction(
            2,
            TransactionOp::CurrencySpend {
                account_id: 3003,
                amount: 10_000,
                is_premium: false,
            },
            20,
            &new_token,
            116,
            &mut journal,
        )
        .expect("New session spend must succeed");

    assert_eq!(lsn, 1);
    assert_eq!(manager.get_account(3003).unwrap().free_currency, 40_000);
}
