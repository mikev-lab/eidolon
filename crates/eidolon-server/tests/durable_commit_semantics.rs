//! Milestone 14.1 Integration Test Suite: Durable Commit Semantics and RPO = 0 Guarantee.
//!
//! Validates that atomic transactions are physically synchronized to disk via POSIX `fdatasync`
//! before client confirmations can be issued, and that sudden process termination or truncated
//! writes leave previously committed transactions intact with zero state corruption.

use std::fs::OpenOptions;
use std::io::Write;

use eidolon_world::durable_journal::{CommitDurability, DurableFileJournal, JOURNAL_MAGIC};
use eidolon_world::transaction::{
    AccountState, TransactionManager, TransactionOp, LOCK_INVENTORY, LOCK_WALLET,
};
use eidolon_world::wal::{WalRecord, WriteAheadJournal};

#[test]
fn test_milestone_14_1_rpo_zero_commit_boundary() {
    let temp_dir = std::env::temp_dir();
    let journal_path = temp_dir.join(format!(
        "eidolon_durability_rpo0_{}.wal",
        std::process::id()
    ));

    let _ = std::fs::remove_file(&journal_path);

    // 1. Initialize transaction manager and persistent journal
    let mut manager = TransactionManager::new();
    let mut account = AccountState::new(1001);
    account.premium_currency = 5000;
    manager.register_account(account);

    let file_sink = DurableFileJournal::open(&journal_path).expect("Open durable journal");
    let mut journal = WriteAheadJournal::<64>::new(Box::new(file_sink));

    let token = manager
        .acquire_lock(1001, LOCK_WALLET, 42, 10, 100, false)
        .expect("Acquire lock");

    // 2. Execute transaction with LocalDiskFsync durability
    let lsn = manager
        .execute_transaction_with_durability(
            1,
            TransactionOp::CurrencySpend {
                account_id: 1001,
                amount: 1500,
                is_premium: true,
            },
            42,
            &token,
            10,
            &mut journal,
            CommitDurability::LocalDiskFsync,
        )
        .expect("Transaction execute");

    assert_eq!(lsn, 1);
    // Durable LSN must be flushed to disk BEFORE returning to caller
    assert_eq!(journal.durable_lsn(), 1);
    assert!(journal.is_flushed(1));
    assert_eq!(manager.get_account(1001).unwrap().premium_currency, 3500);

    // 3. Simulate sudden process crash (drop journal and transaction manager without clean shutdown)
    drop(journal);
    drop(manager);

    // 4. Recover directly from disk file
    let recovered_records =
        DurableFileJournal::recover_from_journal(&journal_path).expect("Recover from journal");
    assert_eq!(
        recovered_records.len(),
        1,
        "Confirmed transaction must exist on disk"
    );
    assert_eq!(recovered_records[0].lsn, 1);
    assert_eq!(recovered_records[0].account_id, 1001);

    // Cleanup
    let _ = std::fs::remove_file(&journal_path);
}

#[test]
fn test_milestone_14_1_recovery_ignores_unacknowledged_truncated_writes() {
    let temp_dir = std::env::temp_dir();
    let journal_path = temp_dir.join(format!(
        "eidolon_durability_trunc_{}.wal",
        std::process::id()
    ));

    let _ = std::fs::remove_file(&journal_path);

    // Write 2 valid committed transactions
    {
        let file_sink = DurableFileJournal::open(&journal_path).expect("Open durable journal");
        let mut journal = WriteAheadJournal::<64>::new(Box::new(file_sink));

        let r1 = WalRecord::new(1, 100, 2001, 1, 0x04, b"deposit:100").unwrap();
        let r2 = WalRecord::new(2, 101, 2001, 1, 0x04, b"deposit:200").unwrap();

        journal.append(100, 2001, 1, 0x04, b"deposit:100").unwrap();
        journal.append(101, 2001, 1, 0x04, b"deposit:200").unwrap();
        journal.flush_pending().unwrap();

        let _ = (r1, r2);
    }

    // Inject simulated mid-write SIGKILL / power loss (corrupted trailing bytes)
    {
        let mut file = OpenOptions::new()
            .append(true)
            .open(&journal_path)
            .expect("Open for inject");

        // Write framing magic and a claimed length of 80 bytes, but only append 12 garbage bytes
        file.write_all(&JOURNAL_MAGIC.to_be_bytes()).unwrap();
        file.write_all(&80u32.to_be_bytes()).unwrap();
        file.write_all(&[
            0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
        ])
        .unwrap();
        file.sync_data().unwrap();
    }

    // Recovery must successfully restore the 2 committed transactions and cleanly drop the incomplete write
    let recovered =
        DurableFileJournal::recover_from_journal(&journal_path).expect("Recover truncated");
    assert_eq!(
        recovered.len(),
        2,
        "Recovery must recover exactly the 2 valid records"
    );
    assert_eq!(recovered[0].lsn, 1);
    assert_eq!(recovered[1].lsn, 2);

    let _ = std::fs::remove_file(&journal_path);
}

#[test]
fn test_milestone_14_1_item_transfer_durability_contract() {
    let temp_dir = std::env::temp_dir();
    let journal_path = temp_dir.join(format!(
        "eidolon_durability_item_{}.wal",
        std::process::id()
    ));

    let _ = std::fs::remove_file(&journal_path);

    let mut manager = TransactionManager::new();
    let mut alice = AccountState::new(3001);
    let bob = AccountState::new(3002);

    alice.add_item(501, 5).expect("Add item to Alice");
    manager.register_account(alice);
    manager.register_account(bob);

    let file_sink = DurableFileJournal::open(&journal_path).expect("Open durable journal");
    let mut journal = WriteAheadJournal::<64>::new(Box::new(file_sink));

    let token = manager
        .acquire_lock(3001, LOCK_INVENTORY, 777, 10, 100, false)
        .expect("Acquire inventory lock");

    // Execute item transfer: Alice transfers 2 of item 501 to Bob
    let lsn = manager
        .execute_transaction_with_durability(
            10,
            TransactionOp::ItemTransfer {
                from_account: 3001,
                to_account: 3002,
                item_id: 501,
                quantity: 2,
            },
            777,
            &token,
            12,
            &mut journal,
            CommitDurability::LocalDiskFsync,
        )
        .expect("Execute item transfer");

    assert_eq!(lsn, 1);
    assert!(journal.is_flushed(1));

    // Verify in-memory balances
    let alice_after = manager.get_account(3001).unwrap();
    let bob_after = manager.get_account(3002).unwrap();
    assert_eq!(alice_after.inventory[0].unwrap().quantity, 3);
    assert_eq!(bob_after.inventory[0].unwrap().quantity, 2);

    // Verify physical persistence
    let recovered =
        DurableFileJournal::recover_from_journal(&journal_path).expect("Recover item journal");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].lsn, 1);
    assert_eq!(recovered[0].account_id, 3001);

    let _ = std::fs::remove_file(&journal_path);
}
