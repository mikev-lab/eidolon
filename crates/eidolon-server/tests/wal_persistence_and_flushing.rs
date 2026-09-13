//! Milestone 9.1: Authoritative Consistency Pipeline and Write-Ahead Journaling (WAL) Tests.
//!
//! Verifies the four-stage consistency pipeline:
//! Simulation Tick -> Authoritative State -> Durable WAL Ring Buffer -> Storage Flush.
//! Proves that the simulation loop never blocks and client ACKs require durable LSN confirmation.

use eidolon_world::wal::{
    MockDurableStorage, WalRecord, WriteAheadJournal, OP_CURRENCY_DELTA, OP_ENTITY_SPAWN,
};

#[test]
fn test_milestone_9_1_four_stage_consistency_pipeline() {
    let mock_storage = Box::new(MockDurableStorage::new());
    let mut journal = WriteAheadJournal::<1024>::new(mock_storage);

    // Stage 1 & 2: Simulation Tick generates authoritative state mutation
    let tick = 100;
    let account_id = 4001;
    let entity_id = 42;
    let mutation_payload = [0xDE, 0xAD, 0xBE, 0xEF];

    // Stage 3: Mutation appended into non-blocking WAL ring buffer
    let lsn = journal
        .append(
            tick,
            account_id,
            entity_id,
            OP_ENTITY_SPAWN,
            &mutation_payload,
        )
        .expect("WAL append must succeed without heap allocation");

    assert_eq!(lsn, 1);
    assert_eq!(journal.current_lsn(), 1);
    assert_eq!(journal.durable_lsn(), 0);

    // Flush acknowledgment gate: client cannot receive ACK before durable flush
    assert!(
        !journal.is_flushed(lsn),
        "Client ACK must be blocked prior to storage flush"
    );
    assert_eq!(journal.pending_count(), 1);

    // Stage 4: Background journal worker flushes records to durable storage
    let flushed_count = journal.flush_pending().expect("Storage flush must succeed");
    assert_eq!(flushed_count, 1);

    // After flush: LSN is durable and client ACK is permitted
    assert_eq!(journal.durable_lsn(), 1);
    assert!(
        journal.is_flushed(lsn),
        "Client ACK must be permitted after durable flush"
    );
    assert_eq!(journal.pending_count(), 0);
}

#[test]
fn test_milestone_9_1_asynchronous_batch_flushing() {
    let mock_storage = Box::new(MockDurableStorage::new());
    let mut journal = WriteAheadJournal::<1024>::new(mock_storage);

    // Simulate 100 rapid mutations within a 20 Hz simulation tick
    let mut lsns = Vec::with_capacity(100);
    for i in 1..=100 {
        let lsn = journal
            .append(200, 5000 + i, i as u32, OP_CURRENCY_DELTA, &[i as u8])
            .expect("Batch append must succeed");
        lsns.push(lsn);
    }

    assert_eq!(journal.current_lsn(), 100);
    assert_eq!(journal.durable_lsn(), 0);
    assert_eq!(journal.pending_count(), 100);

    // Midpoint check: LSN 50 is not yet flushed
    assert!(!journal.is_flushed(50));

    // Background flush drains all 100 records in batch
    let flushed = journal.flush_pending().expect("Batch flush must succeed");
    assert_eq!(flushed, 100);
    assert_eq!(journal.durable_lsn(), 100);

    // All LSNs confirmed durable
    for lsn in lsns {
        assert!(journal.is_flushed(lsn));
    }
}

#[test]
fn test_milestone_9_1_record_binary_integrity_and_checksum() {
    let payload = b"critical_currency_mutation_payload";
    let record = WalRecord::new(500, 120, 99999, 88, OP_CURRENCY_DELTA, payload)
        .expect("Record creation must succeed");

    let mut binary_buf = [0u8; 256];
    let encoded_len = record
        .encode(&mut binary_buf)
        .expect("Binary serialization must succeed");

    // Reconstruct record from raw bytes
    let (decoded, bytes_read) =
        WalRecord::decode(&binary_buf[..encoded_len]).expect("Binary deserialization must succeed");

    assert_eq!(bytes_read, encoded_len);
    assert_eq!(decoded.lsn, 500);
    assert_eq!(decoded.tick, 120);
    assert_eq!(decoded.account_id, 99999);
    assert_eq!(decoded.entity_id, 88);
    assert_eq!(decoded.opcode, OP_CURRENCY_DELTA);
    assert_eq!(
        &decoded.payload[..decoded.payload_len as usize],
        payload.as_slice()
    );
    assert_eq!(decoded.checksum, record.checksum);
}
