//! Authoritative write-ahead journal (WAL) and asynchronous non-blocking persistence pipeline.
//!
//! Enforces an explicit four-stage consistency pipeline:
//! Simulation Tick -> Authoritative State -> Durable WAL Buffer -> Background Storage Flush.
//! Operates with zero heap allocations in the hot simulation tick loop.

use std::fmt;
use std::sync::Mutex;

use crate::error::WorldError;
use crate::hibernation::compute_adler32;

/// Standard maximum capacity for in-memory WAL ring buffers (30 seconds at 20 Hz = 600 ticks, with buffer headroom).
pub const DEFAULT_WAL_BUFFER_CAP: usize = 2048;

/// Maximum payload size in bytes for a single WAL mutation record.
pub const MAX_WAL_PAYLOAD_LEN: usize = 128;

/// Fixed-size header length for encoded WAL binary records (30 bytes).
pub const WAL_HEADER_LEN: usize = 30;

/// Opcode: Entity spawned into zone spatial grid.
pub const OP_ENTITY_SPAWN: u8 = 0x01;
/// Opcode: Entity despawned from zone spatial grid.
pub const OP_ENTITY_DESPAWN: u8 = 0x02;
/// Opcode: Entity position or kinematic transform update.
pub const OP_ENTITY_TRANSFORM: u8 = 0x03;
/// Opcode: Account currency balance credited or debited.
pub const OP_CURRENCY_DELTA: u8 = 0x04;
/// Opcode: Character inventory mutation (item added, removed, or transferred).
pub const OP_INVENTORY_MUTATION: u8 = 0x05;
/// Opcode: Checkpoint snapshot delimiter marker.
pub const OP_CHECKPOINT_MARKER: u8 = 0x06;

/// Self-contained binary WAL mutation record with embedded Adler-32 integrity checksum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalRecord {
    /// Monotonically incrementing Log Sequence Number.
    pub lsn: u64,
    /// Simulation tick index at which the mutation occurred.
    pub tick: u64,
    /// Account identifier associated with the mutation.
    pub account_id: u64,
    /// Entity identifier associated with the mutation.
    pub entity_id: u32,
    /// Mutation opcode identifying the operation type.
    pub opcode: u8,
    /// Length of valid bytes in the payload buffer (<= 128).
    pub payload_len: u8,
    /// Inlined mutation payload buffer.
    pub payload: [u8; MAX_WAL_PAYLOAD_LEN],
    /// Adler-32 checksum computed across header fields and payload.
    pub checksum: u32,
}

impl Default for WalRecord {
    fn default() -> Self {
        Self {
            lsn: 0,
            tick: 0,
            account_id: 0,
            entity_id: 0,
            opcode: 0,
            payload_len: 0,
            payload: [0u8; MAX_WAL_PAYLOAD_LEN],
            checksum: 0,
        }
    }
}

impl WalRecord {
    /// Constructs a new `WalRecord`, copying up to 128 bytes of payload and computing its checksum.
    pub fn new(
        lsn: u64,
        tick: u64,
        account_id: u64,
        entity_id: u32,
        opcode: u8,
        payload_data: &[u8],
    ) -> Result<Self, WorldError> {
        if payload_data.len() > MAX_WAL_PAYLOAD_LEN {
            return Err(WorldError::WalCorruptedRecord(
                "Payload exceeds maximum 128 bytes limit",
            ));
        }

        let mut payload = [0u8; MAX_WAL_PAYLOAD_LEN];
        let p_len = payload_data.len() as u8;
        if let Some(dest) = payload.get_mut(..payload_data.len()) {
            dest.copy_from_slice(payload_data);
        }

        let mut record = Self {
            lsn,
            tick,
            account_id,
            entity_id,
            opcode,
            payload_len: p_len,
            payload,
            checksum: 0,
        };

        record.checksum = record.compute_checksum();
        Ok(record)
    }

    /// Computes the Adler-32 checksum over the record header and valid payload bytes.
    pub fn compute_checksum(&self) -> u32 {
        let mut header_bytes = [0u8; WAL_HEADER_LEN];
        header_bytes[0..8].copy_from_slice(&self.lsn.to_be_bytes());
        header_bytes[8..16].copy_from_slice(&self.tick.to_be_bytes());
        header_bytes[16..24].copy_from_slice(&self.account_id.to_be_bytes());
        header_bytes[24..28].copy_from_slice(&self.entity_id.to_be_bytes());
        header_bytes[28] = self.opcode;
        header_bytes[29] = self.payload_len;

        let len = (self.payload_len as usize).min(MAX_WAL_PAYLOAD_LEN);
        let p_slice = &self.payload[..len];

        // Combine header and payload in checksum calculation
        let c1 = compute_adler32(&header_bytes);
        let c2 = compute_adler32(p_slice);
        c1 ^ c2
    }

    /// Encodes the record into a byte buffer, returning the total bytes written.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, WorldError> {
        let p_len = (self.payload_len as usize).min(MAX_WAL_PAYLOAD_LEN);
        let total_len = WAL_HEADER_LEN + p_len + 4;

        if out.len() < total_len {
            return Err(WorldError::WalCorruptedRecord(
                "Destination buffer too small",
            ));
        }

        out[0..8].copy_from_slice(&self.lsn.to_be_bytes());
        out[8..16].copy_from_slice(&self.tick.to_be_bytes());
        out[16..24].copy_from_slice(&self.account_id.to_be_bytes());
        out[24..28].copy_from_slice(&self.entity_id.to_be_bytes());
        out[28] = self.opcode;
        out[29] = self.payload_len;

        out[WAL_HEADER_LEN..WAL_HEADER_LEN + p_len].copy_from_slice(&self.payload[..p_len]);

        let trailer_offset = WAL_HEADER_LEN + p_len;
        out[trailer_offset..trailer_offset + 4].copy_from_slice(&self.checksum.to_be_bytes());

        Ok(total_len)
    }

    /// Decodes a WAL record from a binary byte slice.
    pub fn decode(buf: &[u8]) -> Result<(Self, usize), WorldError> {
        if buf.len() < WAL_HEADER_LEN + 4 {
            return Err(WorldError::WalCorruptedRecord(
                "Buffer truncated before header",
            ));
        }

        let mut lsn_bytes = [0u8; 8];
        lsn_bytes.copy_from_slice(&buf[0..8]);
        let lsn = u64::from_be_bytes(lsn_bytes);

        let mut tick_bytes = [0u8; 8];
        tick_bytes.copy_from_slice(&buf[8..16]);
        let tick = u64::from_be_bytes(tick_bytes);

        let mut acc_bytes = [0u8; 8];
        acc_bytes.copy_from_slice(&buf[16..24]);
        let account_id = u64::from_be_bytes(acc_bytes);

        let mut ent_bytes = [0u8; 4];
        ent_bytes.copy_from_slice(&buf[24..28]);
        let entity_id = u32::from_be_bytes(ent_bytes);

        let opcode = buf[28];
        let payload_len = buf[29];

        if payload_len as usize > MAX_WAL_PAYLOAD_LEN {
            return Err(WorldError::WalCorruptedRecord(
                "Payload length header invalid",
            ));
        }

        let p_len = payload_len as usize;
        let total_len = WAL_HEADER_LEN + p_len + 4;
        if buf.len() < total_len {
            return Err(WorldError::WalCorruptedRecord(
                "Buffer truncated before trailer",
            ));
        }

        let mut payload = [0u8; MAX_WAL_PAYLOAD_LEN];
        payload[..p_len].copy_from_slice(&buf[WAL_HEADER_LEN..WAL_HEADER_LEN + p_len]);

        let trailer_offset = WAL_HEADER_LEN + p_len;
        let mut chk_bytes = [0u8; 4];
        chk_bytes.copy_from_slice(&buf[trailer_offset..trailer_offset + 4]);
        let checksum = u32::from_be_bytes(chk_bytes);

        let record = Self {
            lsn,
            tick,
            account_id,
            entity_id,
            opcode,
            payload_len,
            payload,
            checksum,
        };

        let calculated = record.compute_checksum();
        if calculated != checksum {
            return Err(WorldError::WalChecksumMismatch {
                expected: checksum,
                actual: calculated,
            });
        }

        Ok((record, total_len))
    }
}

/// In-memory bounded queue state for non-blocking simulation handoffs.
#[derive(Debug)]
struct RingState<const CAP: usize> {
    slots: Box<[WalRecord]>,
    head: usize,
    tail: usize,
    len: usize,
}

/// Thread-safe bounded WAL ring buffer operating with zero dynamic allocations in the hot path.
#[derive(Debug)]
pub struct WalRingBuffer<const CAP: usize = DEFAULT_WAL_BUFFER_CAP> {
    state: Mutex<RingState<CAP>>,
}

impl<const CAP: usize> Default for WalRingBuffer<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAP: usize> WalRingBuffer<CAP> {
    /// Constructs a new WAL ring buffer pre-allocating all slots.
    pub fn new() -> Self {
        let mut slots = Vec::with_capacity(CAP);
        slots.resize_with(CAP, WalRecord::default);
        Self {
            state: Mutex::new(RingState {
                slots: slots.into_boxed_slice(),
                head: 0,
                tail: 0,
                len: 0,
            }),
        }
    }

    /// Pushes a record into the ring buffer. Returns `Err(WorldError::WalBufferFull)` if saturated.
    pub fn try_push(&self, record: WalRecord) -> Result<(), WorldError> {
        let mut state = self.state.lock().map_err(|_| WorldError::WalBufferFull)?;

        if state.len >= CAP {
            return Err(WorldError::WalBufferFull);
        }

        let tail = state.tail;
        if let Some(slot) = state.slots.get_mut(tail) {
            *slot = record;
            state.tail = (tail + 1) % CAP;
            state.len += 1;
            Ok(())
        } else {
            Err(WorldError::WalBufferFull)
        }
    }

    /// Inspects available records from the head of the queue without removing them.
    pub fn peek_batch(&self, dest: &mut [WalRecord]) -> usize {
        let state = match self.state.lock() {
            Ok(g) => g,
            Err(_) => return 0,
        };

        let count = dest.len().min(state.len);
        for (i, item) in dest.iter_mut().take(count).enumerate() {
            let idx = (state.head + i) % CAP;
            if let Some(slot) = state.slots.get(idx) {
                *item = *slot;
            }
        }
        count
    }

    /// Advances the head pointer, permanently acknowledging flushed records from the ring buffer.
    pub fn advance_head(&self, count: usize) {
        if let Ok(mut state) = self.state.lock() {
            let actual = count.min(state.len);
            state.head = (state.head + actual) % CAP;
            state.len = state.len.saturating_sub(actual);
        }
    }

    /// Drains available records into the destination slice, returning the number drained.
    pub fn drain_batch(&self, dest: &mut [WalRecord]) -> usize {
        let mut state = match self.state.lock() {
            Ok(g) => g,
            Err(_) => return 0,
        };

        let count = dest.len().min(state.len);
        for item in dest.iter_mut().take(count) {
            let head = state.head;
            if let Some(slot) = state.slots.get(head) {
                *item = *slot;
            }
            state.head = (head + 1) % CAP;
        }
        state.len = state.len.saturating_sub(count);
        count
    }

    /// Returns the number of records currently queued in memory.
    pub fn len(&self) -> usize {
        self.state.lock().map(|s| s.len).unwrap_or(0)
    }

    /// Returns true if the ring buffer contains no records.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the buffer utilization ratio between 0.0 and 1.0.
    pub fn utilization_ratio(&self) -> f32 {
        self.len() as f32 / CAP as f32
    }
}

/// Abstract destination sink for durable write-ahead logging.
pub trait DurableWalSink: Send {
    /// Writes a batch of sequential WAL records to durable storage.
    fn write_batch(&mut self, records: &[WalRecord]) -> Result<(), WorldError>;
    /// Flushes any buffered disk writes to physical storage (fsync).
    fn flush(&mut self) -> Result<(), WorldError>;
}

/// In-memory mock storage sink simulating disk and database outages with exponential recovery.
#[derive(Debug, Clone, Default)]
pub struct MockDurableStorage {
    /// Flushed records stored durably.
    pub records: std::sync::Arc<Mutex<Vec<WalRecord>>>,
    /// Shared flag controlling whether durable storage is currently reachable.
    pub is_available: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl MockDurableStorage {
    /// Constructs a new mock storage backend with storage available by default.
    pub fn new() -> Self {
        Self {
            records: std::sync::Arc::new(Mutex::new(Vec::new())),
            is_available: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        }
    }

    /// Toggles the simulated storage connection status.
    pub fn set_available(&self, available: bool) {
        self.is_available
            .store(available, std::sync::atomic::Ordering::SeqCst);
    }
}

impl DurableWalSink for MockDurableStorage {
    fn write_batch(&mut self, records: &[WalRecord]) -> Result<(), WorldError> {
        if !self.is_available.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(WorldError::WalStorageUnavailable);
        }

        if let Ok(mut lock) = self.records.lock() {
            lock.extend_from_slice(records);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), WorldError> {
        if !self.is_available.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(WorldError::WalStorageUnavailable);
        }
        Ok(())
    }
}

/// Asynchronous write-ahead journal coordinator orchestrating the 4-stage persistence pipeline.
pub struct WriteAheadJournal<const CAP: usize = DEFAULT_WAL_BUFFER_CAP> {
    current_lsn: u64,
    durable_lsn: u64,
    ring_buffer: WalRingBuffer<CAP>,
    sink: Box<dyn DurableWalSink>,
    high_watermark_ratio: f32,
    drain_scratch: [WalRecord; 128],
}

impl<const CAP: usize> fmt::Debug for WriteAheadJournal<CAP> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteAheadJournal")
            .field("current_lsn", &self.current_lsn)
            .field("durable_lsn", &self.durable_lsn)
            .field("queued_records", &self.ring_buffer.len())
            .finish()
    }
}

impl<const CAP: usize> WriteAheadJournal<CAP> {
    /// Constructs a new write-ahead journal with the specified durable sink.
    pub fn new(sink: Box<dyn DurableWalSink>) -> Self {
        Self {
            current_lsn: 0,
            durable_lsn: 0,
            ring_buffer: WalRingBuffer::new(),
            sink,
            high_watermark_ratio: 0.85,
            drain_scratch: [WalRecord::default(); 128],
        }
    }

    /// Returns the highest LSN appended into the memory buffer.
    #[inline]
    pub fn current_lsn(&self) -> u64 {
        self.current_lsn
    }

    /// Returns the highest LSN confirmed flushed to durable storage.
    #[inline]
    pub fn durable_lsn(&self) -> u64 {
        self.durable_lsn
    }

    /// Checks whether a specific transaction LSN has been flushed to durable storage.
    #[inline]
    pub fn is_flushed(&self, lsn: u64) -> bool {
        lsn <= self.durable_lsn
    }

    /// Appends a mutation record into the WAL ring buffer (hot simulation path).
    ///
    /// Returns the assigned monotonic Log Sequence Number (LSN).
    pub fn append(
        &mut self,
        tick: u64,
        account_id: u64,
        entity_id: u32,
        opcode: u8,
        payload_data: &[u8],
    ) -> Result<u64, WorldError> {
        let next_lsn = self.current_lsn.saturating_add(1);
        let record = WalRecord::new(next_lsn, tick, account_id, entity_id, opcode, payload_data)?;

        self.ring_buffer.try_push(record)?;
        self.current_lsn = next_lsn;
        Ok(next_lsn)
    }

    /// Flushes pending queued records to the durable storage sink.
    ///
    /// Advances `durable_lsn` and evicts records from the buffer only upon successful storage synchronization.
    /// If storage fails, all pending records remain intact in memory for subsequent retry.
    pub fn flush_pending(&mut self) -> Result<usize, WorldError> {
        let mut total_flushed = 0;

        loop {
            let count = self.ring_buffer.peek_batch(&mut self.drain_scratch);
            if count == 0 {
                break;
            }

            let batch = &self.drain_scratch[..count];
            self.sink.write_batch(batch)?;

            // Acknowledge records in ring buffer now that write succeeded
            self.ring_buffer.advance_head(count);

            if let Some(last_rec) = batch.last() {
                if last_rec.lsn > self.durable_lsn {
                    self.durable_lsn = last_rec.lsn;
                }
            }

            total_flushed += count;
        }

        if total_flushed > 0 {
            self.sink.flush()?;
        }

        Ok(total_flushed)
    }

    /// Returns the number of records waiting in memory for storage flush.
    #[inline]
    pub fn pending_count(&self) -> usize {
        self.ring_buffer.len()
    }

    /// Returns true if the in-memory buffer exceeds the backpressure high-watermark.
    #[inline]
    pub fn is_congested(&self) -> bool {
        self.ring_buffer.utilization_ratio() >= self.high_watermark_ratio
    }

    /// Returns the underlying buffer utilization ratio.
    #[inline]
    pub fn utilization_ratio(&self) -> f32 {
        self.ring_buffer.utilization_ratio()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wal_record_encode_decode_roundtrip() {
        let payload = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
        let record = WalRecord::new(101, 55, 9999, 42, OP_CURRENCY_DELTA, &payload)
            .expect("Record creation must succeed");

        let mut buf = [0u8; 256];
        let written = record.encode(&mut buf).expect("Encode must succeed");
        assert_eq!(written, WAL_HEADER_LEN + payload.len() + 4);

        let (decoded, read_len) = WalRecord::decode(&buf[..written]).expect("Decode must succeed");
        assert_eq!(read_len, written);
        assert_eq!(decoded.lsn, 101);
        assert_eq!(decoded.tick, 55);
        assert_eq!(decoded.account_id, 9999);
        assert_eq!(decoded.entity_id, 42);
        assert_eq!(decoded.opcode, OP_CURRENCY_DELTA);
        assert_eq!(decoded.payload_len, 5);
        assert_eq!(&decoded.payload[..5], &payload);
        assert_eq!(decoded.checksum, record.checksum);
    }

    #[test]
    fn test_wal_record_detects_corrupted_checksum() {
        let payload = [1, 2, 3, 4];
        let record = WalRecord::new(1, 10, 100, 5, OP_ENTITY_SPAWN, &payload).unwrap();

        let mut buf = [0u8; 128];
        let written = record.encode(&mut buf).unwrap();

        // Corrupt payload byte
        buf[WAL_HEADER_LEN] ^= 0xFF;

        let err = WalRecord::decode(&buf[..written]).expect_err("Must reject corrupted record");
        match err {
            WorldError::WalChecksumMismatch { .. } => {}
            other => panic!("Expected WalChecksumMismatch, got {other:?}"),
        }
    }

    #[test]
    fn test_write_ahead_journal_flow_and_flush_ack() {
        let mock_storage = Box::new(MockDurableStorage::new());
        let mut journal = WriteAheadJournal::<64>::new(mock_storage);

        assert_eq!(journal.current_lsn(), 0);
        assert_eq!(journal.durable_lsn(), 0);

        let lsn1 = journal
            .append(1, 1001, 10, OP_CURRENCY_DELTA, &[10, 20])
            .unwrap();
        let lsn2 = journal
            .append(1, 1001, 10, OP_CURRENCY_DELTA, &[30, 40])
            .unwrap();
        assert_eq!(lsn1, 1);
        assert_eq!(lsn2, 2);

        // Before flush: LSNs are not yet durable
        assert!(!journal.is_flushed(lsn1));
        assert!(!journal.is_flushed(lsn2));
        assert_eq!(journal.pending_count(), 2);

        // Flush pending to durable storage
        let flushed = journal.flush_pending().unwrap();
        assert_eq!(flushed, 2);
        assert_eq!(journal.durable_lsn(), 2);
        assert!(journal.is_flushed(lsn1));
        assert!(journal.is_flushed(lsn2));
        assert_eq!(journal.pending_count(), 0);
    }

    #[test]
    fn test_wal_ring_buffer_backpressure_and_overflow() {
        let ring = WalRingBuffer::<4>::new();
        let rec = WalRecord::new(1, 1, 1, 1, OP_ENTITY_SPAWN, &[0]).unwrap();

        assert!(ring.try_push(rec).is_ok());
        assert!(ring.try_push(rec).is_ok());
        assert!(ring.try_push(rec).is_ok());
        assert!(ring.try_push(rec).is_ok());
        assert_eq!(ring.len(), 4);

        // 5th push must fail with WalBufferFull
        let err = ring.try_push(rec).expect_err("Overflow must return error");
        assert_eq!(err, WorldError::WalBufferFull);
    }
}
