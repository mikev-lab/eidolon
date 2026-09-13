//! Milestone 9.3: Asynchronous Database and Storage Outage Resilience Tests.
//!
//! Simulates prolonged (30-second / 600-tick) storage unavailability.
//! Verifies that the 20 Hz simulation loop runs uninterrupted, mutations buffer in memory,
//! and all records replay and synchronize durably upon backend recovery.

use eidolon_world::wal::{MockDurableStorage, WriteAheadJournal, OP_CURRENCY_DELTA};
use eidolon_world::WorldError;

#[test]
fn test_milestone_9_3_thirty_second_storage_outage_resilience() {
    let storage_mock = MockDurableStorage::new();
    let storage_ctrl = storage_mock.clone();
    let mut journal = WriteAheadJournal::<2048>::new(Box::new(storage_mock));

    // Warm-up: 10 normal ticks with immediate flushing
    for tick in 1..=10 {
        journal
            .append(tick, 1000 + tick, 1, OP_CURRENCY_DELTA, &[1])
            .expect("Warm-up append must succeed");
    }
    let flushed_warmup = journal.flush_pending().expect("Warm-up flush");
    assert_eq!(flushed_warmup, 10);
    assert_eq!(journal.durable_lsn(), 10);
    assert_eq!(journal.pending_count(), 0);

    // OUTAGE EVENT: Simulate storage/database becoming unreachable for 30 seconds (600 ticks)
    storage_ctrl.set_available(false);

    // Simulate 600 ticks (30 seconds at 20 Hz) with 1 mutation per tick
    let mut outage_lsns = Vec::with_capacity(600);
    for tick in 11..=610 {
        let lsn = journal
            .append(tick, 2000 + tick, 1, OP_CURRENCY_DELTA, &[tick as u8])
            .expect("Simulation loop append must never block or fail during outage");
        outage_lsns.push(lsn);

        // Attempting flush during outage fails gracefully with typed error
        let flush_res = journal.flush_pending();
        assert_eq!(flush_res.unwrap_err(), WorldError::WalStorageUnavailable);
    }

    assert_eq!(journal.current_lsn(), 610);
    assert_eq!(journal.durable_lsn(), 10);
    assert_eq!(journal.pending_count(), 600);

    // Invariant: Simulation tick loop was never halted, buffer did not crash
    assert!(!journal.is_congested()); // 600 / 2048 < 0.85
    assert_eq!(outage_lsns.len(), 600);

    // RECOVERY EVENT: Storage backend reconnects
    storage_ctrl.set_available(true);

    // Flush catches up all 600 records in batch
    let recovered_flushed = journal
        .flush_pending()
        .expect("Post-outage flush must succeed");
    assert_eq!(recovered_flushed, 600);
    assert_eq!(journal.durable_lsn(), 610);
    assert_eq!(journal.pending_count(), 0);

    // Verify all 610 LSNs are now durable
    for lsn in 1..=610 {
        assert!(journal.is_flushed(lsn));
    }
}
