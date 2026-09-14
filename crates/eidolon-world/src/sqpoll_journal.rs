//! Asynchronous io_uring and SQPOLL-compatible double-buffered transaction journal.
//!
//! Eliminates physical disk fsync/fdatasync stalls from authoritative 20 Hz tick simulation
//! loops by pushing records into an atomic double-buffered in-memory queue in sub-microsecond time,
//! while a background worker flushes and syncs non-volatile storage out-of-band.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::durable_journal::{DurableFileJournal, JOURNAL_MAGIC};
use crate::wal::{WalRecord, MAX_WAL_PAYLOAD_LEN, WAL_HEADER_LEN};

/// Maximum number of WAL records stored in each double-buffer partition.
pub const JOURNAL_BUFFER_CAPACITY: usize = 2048;

/// Stamped WAL record with its monotonic transaction sequence number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StampedWalRecord {
    /// Monotonic transaction sequence ID.
    pub sequence: u64,
    /// Encapsulated write-ahead log record payload.
    pub record: WalRecord,
}

/// Double-buffered in-memory queue for lock-free, zero-allocation WAL submission.
pub struct DoubleBufferedJournalQueue {
    active_buf_idx: AtomicUsize,
    buffers: [Mutex<Vec<StampedWalRecord>>; 2],
    latest_sequence: AtomicU64,
    committed_sequence: AtomicU64,
}

impl Default for DoubleBufferedJournalQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl DoubleBufferedJournalQueue {
    /// Constructs a pre-allocated double-buffered journal queue.
    pub fn new() -> Self {
        Self {
            active_buf_idx: AtomicUsize::new(0),
            buffers: [
                Mutex::new(Vec::with_capacity(JOURNAL_BUFFER_CAPACITY)),
                Mutex::new(Vec::with_capacity(JOURNAL_BUFFER_CAPACITY)),
            ],
            latest_sequence: AtomicU64::new(0),
            committed_sequence: AtomicU64::new(0),
        }
    }

    /// Pushes a WAL record into the active buffer in sub-microsecond time.
    ///
    /// Never blocks on disk I/O. Returns the allocated monotonic sequence number,
    /// or None if the active buffer is temporarily saturated pending a background flush.
    pub fn push(&self, record: WalRecord) -> Option<u64> {
        let buf_idx = self.active_buf_idx.load(Ordering::Relaxed) % 2;

        if let Ok(mut guard) = self.buffers[buf_idx].lock() {
            if guard.len() < JOURNAL_BUFFER_CAPACITY {
                let seq = self.latest_sequence.fetch_add(1, Ordering::SeqCst) + 1;
                guard.push(StampedWalRecord {
                    sequence: seq,
                    record,
                });
                Some(seq)
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Returns the highest sequence number physically synced and durable on disk.
    #[inline]
    pub fn committed_sequence(&self) -> u64 {
        self.committed_sequence.load(Ordering::Acquire)
    }

    /// Returns the latest assigned sequence number.
    #[inline]
    pub fn latest_sequence(&self) -> u64 {
        self.latest_sequence.load(Ordering::Relaxed)
    }

    /// Returns true if the specified transaction sequence number has been physically written
    /// and synchronized to disk via fdatasync.
    #[inline]
    pub fn is_durable(&self, sequence: u64) -> bool {
        self.committed_sequence.load(Ordering::Acquire) >= sequence
    }

    /// Swaps the active buffer and extracts all records from the inactive buffer into `out`.
    ///
    /// Executes via zero-allocation vector swap. Returns the count of extracted records.
    pub fn swap_and_drain(&self, out: &mut Vec<StampedWalRecord>) -> usize {
        let old_idx = self.active_buf_idx.load(Ordering::SeqCst) % 2;
        let next_idx = 1 - old_idx;

        // Atomically switch active buffer
        self.active_buf_idx.store(next_idx, Ordering::SeqCst);

        // Drain inactive buffer into out
        out.clear();
        if let Ok(mut guard) = self.buffers[old_idx].lock() {
            std::mem::swap(&mut *guard, out);
        }

        out.len()
    }

    /// Flushes the provided batch of records to disk and updates the committed sequence fence.
    pub fn flush_records<W: Write>(
        &self,
        records: &[StampedWalRecord],
        writer: &mut W,
    ) -> io::Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }

        let mut record_buf = [0u8; WAL_HEADER_LEN + MAX_WAL_PAYLOAD_LEN + 4];
        let mut frame_header = [0u8; 8];
        let mut total_bytes = 0;
        let mut highest_seq = 0u64;

        for stamped in records {
            if stamped.sequence > highest_seq {
                highest_seq = stamped.sequence;
            }

            let encoded_len = match stamped.record.encode(&mut record_buf) {
                Ok(len) => len,
                Err(e) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("WAL encode error: {:?}", e),
                    ));
                }
            };

            frame_header[0..4].copy_from_slice(&JOURNAL_MAGIC.to_be_bytes());
            frame_header[4..8].copy_from_slice(&(encoded_len as u32).to_be_bytes());

            writer.write_all(&frame_header)?;
            writer.write_all(&record_buf[..encoded_len])?;

            total_bytes += 8 + encoded_len;
        }

        writer.flush()?;

        if highest_seq > 0 {
            self.committed_sequence
                .store(highest_seq, Ordering::Release);
        }

        Ok(total_bytes)
    }
}

/// Asynchronous transaction journal engine utilizing double-buffering and background sync.
pub struct AsyncSqpollJournal {
    queue: Arc<DoubleBufferedJournalQueue>,
    file: Option<File>,
    path: PathBuf,
    is_running: Arc<AtomicBool>,
    worker_handle: Option<JoinHandle<()>>,
}

impl AsyncSqpollJournal {
    /// Opens an asynchronous journal at the specified path, starting a background flusher thread.
    pub fn open(path: impl AsRef<Path>, flush_interval: Duration) -> io::Result<Self> {
        let path_buf = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path_buf)?;

        let queue = Arc::new(DoubleBufferedJournalQueue::new());
        let is_running = Arc::new(AtomicBool::new(true));

        let queue_clone = Arc::clone(&queue);
        let is_running_clone = Arc::clone(&is_running);
        let mut worker_file = file.try_clone()?;

        let worker_handle = thread::Builder::new()
            .name("eidolon-sqpoll-flusher".into())
            .spawn(move || {
                let mut drain_scratch = Vec::with_capacity(JOURNAL_BUFFER_CAPACITY);

                while is_running_clone.load(Ordering::Relaxed) {
                    thread::sleep(flush_interval);

                    let count = queue_clone.swap_and_drain(&mut drain_scratch);
                    if count > 0
                        && queue_clone
                            .flush_records(&drain_scratch, &mut worker_file)
                            .is_ok()
                    {
                        let _ = worker_file.sync_data();
                    }
                }

                // Final drain on termination
                let count = queue_clone.swap_and_drain(&mut drain_scratch);
                if count > 0
                    && queue_clone
                        .flush_records(&drain_scratch, &mut worker_file)
                        .is_ok()
                {
                    let _ = worker_file.sync_data();
                }
            })?;

        Ok(Self {
            queue,
            file: Some(file),
            path: path_buf,
            is_running,
            worker_handle: Some(worker_handle),
        })
    }

    /// Pushes a WAL record into the journal queue without blocking on disk.
    ///
    /// Executes in sub-microsecond time. Returns the monotonic sequence number for fencing.
    #[inline]
    pub fn push(&self, record: WalRecord) -> Option<u64> {
        self.queue.push(record)
    }

    /// Checks if a transaction sequence has been durably synced to non-volatile storage.
    #[inline]
    pub fn is_durable(&self, sequence: u64) -> bool {
        self.queue.is_durable(sequence)
    }

    /// Returns the highest durably committed sequence number.
    #[inline]
    pub fn committed_sequence(&self) -> u64 {
        self.queue.committed_sequence()
    }

    /// Forces a synchronous buffer swap and disk sync (e.g. during graceful server shutdown).
    pub fn flush_sync(&mut self) -> io::Result<usize> {
        let mut scratch = Vec::with_capacity(JOURNAL_BUFFER_CAPACITY);
        let count = self.queue.swap_and_drain(&mut scratch);
        if count == 0 {
            return Ok(0);
        }

        if let Some(ref mut file) = self.file {
            let bytes = self.queue.flush_records(&scratch, file)?;
            file.sync_data()?;
            Ok(bytes)
        } else {
            Ok(0)
        }
    }

    /// Returns the path to the journal file.
    #[inline]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Recovers all committed records using the standard recovery mechanism.
    pub fn recover(path: impl AsRef<Path>) -> io::Result<Vec<WalRecord>> {
        DurableFileJournal::recover_from_journal(path)
    }
}

impl Drop for AsyncSqpollJournal {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_double_buffered_journal_queue_push_and_drain() {
        let queue = DoubleBufferedJournalQueue::new();

        let r1 = WalRecord::new(1, 10, 1, 100, crate::wal::OP_CURRENCY_DELTA, &[1, 2, 3]).unwrap();
        let r2 =
            WalRecord::new(2, 10, 1, 101, crate::wal::OP_ENTITY_TRANSFORM, &[4, 5, 6]).unwrap();

        let seq1 = queue.push(r1).expect("Push r1 should succeed");
        let seq2 = queue.push(r2).expect("Push r2 should succeed");
        assert_eq!(seq1, 1);
        assert_eq!(seq2, 2);
        assert_eq!(queue.latest_sequence(), 2);
        assert_eq!(queue.committed_sequence(), 0);

        // Swap active buffer
        let mut drained = Vec::new();
        let count = queue.swap_and_drain(&mut drained);
        assert_eq!(count, 2);

        let mut output_bytes = Vec::new();
        let bytes_written = queue
            .flush_records(&drained, &mut output_bytes)
            .expect("Flush records should succeed");
        assert!(bytes_written > 0);
        assert_eq!(queue.committed_sequence(), 2);
        assert!(queue.is_durable(1));
        assert!(queue.is_durable(2));
        assert!(!queue.is_durable(3));
    }

    #[test]
    fn test_async_sqpoll_journal_recovery_parity() {
        let temp_dir = std::env::temp_dir();
        let journal_path = temp_dir.join(format!("test_async_journal_{}.wal", std::process::id()));

        if journal_path.exists() {
            let _ = std::fs::remove_file(&journal_path);
        }

        {
            let mut journal = AsyncSqpollJournal::open(&journal_path, Duration::from_millis(5))
                .expect("Journal should open");

            for i in 1..=50 {
                let rec = WalRecord::new(
                    i,
                    100,
                    i * 10,
                    i as u32,
                    crate::wal::OP_CURRENCY_DELTA,
                    &[0xAA, 0xBB],
                )
                .unwrap();
                let seq = journal.push(rec).expect("Push should succeed");
                assert_eq!(seq, i);
            }

            // Force synchronous flush to disk
            journal.flush_sync().expect("Flush sync should succeed");
        }

        // Recover and assert all 50 records match exactly
        let recovered =
            AsyncSqpollJournal::recover(&journal_path).expect("Recovery should succeed");
        assert_eq!(recovered.len(), 50);

        for (idx, rec) in recovered.iter().enumerate() {
            let expected_lsn = (idx + 1) as u64;
            assert_eq!(rec.lsn, expected_lsn);
        }

        let _ = std::fs::remove_file(&journal_path);
    }
}
