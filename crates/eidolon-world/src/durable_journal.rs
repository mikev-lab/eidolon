//! Append-only durable file journal with synchronous physical disk synchronization.
//!
//! Provides the physical durability contract for economic and persistent operations:
//! `CommitDurability::LocalDiskFsync` guarantees POSIX `fdatasync` execution before the server
//! acknowledges an operation to the client, delivering RPO = 0 under process `SIGKILL` or host crash.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::wal::{WalRecord, MAX_WAL_PAYLOAD_LEN, WAL_HEADER_LEN};

/// 4-byte file header magic number identifying an eidolon durable journal (`E1D0JOUR`).
pub const JOURNAL_MAGIC: u32 = 0xE1D0_4A52;

/// Physical durability guarantee required for a state transition or transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommitDurability {
    /// In-memory buffering only (zero disk I/O; suitable for 20 Hz movement and transient entity state).
    #[default]
    InMemoryBuffered,
    /// Synchronous POSIX `fdatasync` to local append-only journal file prior to client acknowledgment (RPO = 0).
    LocalDiskFsync,
    /// Synchronous multi-node distributed quorum confirmation.
    DistributedQuorum,
}

/// Append-only persistent file journal enforcing atomic disk flushes.
#[derive(Debug)]
pub struct DurableFileJournal {
    file: File,
    path: PathBuf,
    records_written: u64,
    bytes_written: u64,
}

impl DurableFileJournal {
    /// Opens or creates a durable append-only journal file at the specified path.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path_buf = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path_buf)?;

        let metadata = file.metadata()?;
        let initial_bytes = metadata.len();

        Ok(Self {
            file,
            path: path_buf,
            records_written: 0,
            bytes_written: initial_bytes,
        })
    }

    /// Appends a WAL record to the journal file and physically synchronizes to disk via `fdatasync`.
    ///
    /// The function blocks until the operating system guarantees the bytes reside on non-volatile storage.
    /// Returns the total bytes written for this record.
    pub fn append_and_sync(&mut self, record: &WalRecord) -> io::Result<usize> {
        let mut record_buf = [0u8; WAL_HEADER_LEN + MAX_WAL_PAYLOAD_LEN + 4];
        let encoded_len = record.encode(&mut record_buf).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("WAL encoding failure: {:?}", e),
            )
        })?;

        // 8-byte framing header: 4-byte Magic + 4-byte Record Length
        let mut frame_header = [0u8; 8];
        frame_header[0..4].copy_from_slice(&JOURNAL_MAGIC.to_be_bytes());
        frame_header[4..8].copy_from_slice(&(encoded_len as u32).to_be_bytes());

        self.file.write_all(&frame_header)?;
        self.file.write_all(&record_buf[..encoded_len])?;

        // Physical POSIX fdatasync: flushes dirty disk caches to non-volatile storage
        self.file.sync_data()?;

        let total_written = 8 + encoded_len;
        self.records_written += 1;
        self.bytes_written += total_written as u64;

        Ok(total_written)
    }

    /// Recovers all valid committed WAL records from a durable journal file.
    ///
    /// Replays records sequentially, gracefully halting at EOF or at the first partially-written
    /// or CRC-corrupted entry (e.g. caused by a sudden process termination before `fdatasync`).
    pub fn recover_from_journal(path: impl AsRef<Path>) -> io::Result<Vec<WalRecord>> {
        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };

        file.seek(SeekFrom::Start(0))?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;

        let mut records = Vec::new();
        let mut cursor = 0;

        while cursor + 8 <= buffer.len() {
            let magic = u32::from_be_bytes([
                buffer[cursor],
                buffer[cursor + 1],
                buffer[cursor + 2],
                buffer[cursor + 3],
            ]);

            if magic != JOURNAL_MAGIC {
                // Invalid or corrupted magic: halt recovery at last valid boundary
                break;
            }

            let record_len = u32::from_be_bytes([
                buffer[cursor + 4],
                buffer[cursor + 5],
                buffer[cursor + 6],
                buffer[cursor + 7],
            ]) as usize;

            cursor += 8;

            if cursor + record_len > buffer.len() {
                // Truncated trailing record from abrupt SIGKILL: discard incomplete write
                break;
            }

            match WalRecord::decode(&buffer[cursor..cursor + record_len]) {
                Ok((rec, consumed)) => {
                    records.push(rec);
                    cursor += consumed;
                }
                Err(_) => {
                    // Checksum mismatch or corruption: halt recovery at clean point
                    break;
                }
            }
        }

        Ok(records)
    }

    /// Returns the filesystem path to the journal file.
    #[inline]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the count of records appended in this session.
    #[inline]
    pub const fn records_written(&self) -> u64 {
        self.records_written
    }

    /// Returns total file size in bytes.
    #[inline]
    pub const fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

impl crate::wal::DurableWalSink for DurableFileJournal {
    fn write_batch(&mut self, records: &[WalRecord]) -> Result<(), crate::error::WorldError> {
        let mut record_buf = [0u8; WAL_HEADER_LEN + MAX_WAL_PAYLOAD_LEN + 4];
        for record in records {
            let encoded_len = record.encode(&mut record_buf)?;
            let mut frame_header = [0u8; 8];
            frame_header[0..4].copy_from_slice(&JOURNAL_MAGIC.to_be_bytes());
            frame_header[4..8].copy_from_slice(&(encoded_len as u32).to_be_bytes());

            self.file
                .write_all(&frame_header)
                .map_err(|_| crate::error::WorldError::WalStorageUnavailable)?;
            self.file
                .write_all(&record_buf[..encoded_len])
                .map_err(|_| crate::error::WorldError::WalStorageUnavailable)?;

            self.records_written += 1;
            self.bytes_written += (8 + encoded_len) as u64;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), crate::error::WorldError> {
        self.file
            .sync_data()
            .map_err(|_| crate::error::WorldError::WalStorageUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_durable_file_journal_append_and_recover() {
        let temp_dir = std::env::temp_dir();
        let journal_path =
            temp_dir.join(format!("eidolon_test_journal_{}.wal", std::process::id()));

        // Clean up prior runs if any
        let _ = std::fs::remove_file(&journal_path);

        {
            let mut journal = DurableFileJournal::open(&journal_path).expect("Journal open");
            assert_eq!(journal.records_written(), 0);

            let r1 = WalRecord::new(1, 100, 1001, 10, 0x04, b"deposit:500").unwrap();
            let r2 = WalRecord::new(2, 101, 1001, 10, 0x04, b"withdraw:200").unwrap();

            let bytes1 = journal.append_and_sync(&r1).expect("Append 1");
            let bytes2 = journal.append_and_sync(&r2).expect("Append 2");

            assert!(bytes1 > 0);
            assert!(bytes2 > 0);
            assert_eq!(journal.records_written(), 2);
        }

        // Recover from disk
        let recovered = DurableFileJournal::recover_from_journal(&journal_path).expect("Recover");
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered[0].lsn, 1);
        assert_eq!(recovered[1].lsn, 2);
        assert_eq!(recovered[0].account_id, 1001);

        // Cleanup
        let _ = std::fs::remove_file(&journal_path);
    }

    #[test]
    fn test_durable_file_journal_handles_truncated_trailing_record() {
        let temp_dir = std::env::temp_dir();
        let journal_path = temp_dir.join(format!(
            "eidolon_test_trunc_journal_{}.wal",
            std::process::id()
        ));

        let _ = std::fs::remove_file(&journal_path);

        {
            let mut journal = DurableFileJournal::open(&journal_path).expect("Journal open");
            let r1 = WalRecord::new(1, 100, 2001, 20, 0x04, b"trade:1").unwrap();
            journal.append_and_sync(&r1).expect("Append 1");
        }

        // Inject simulated partial write (e.g. power loss / SIGKILL mid-append)
        {
            let mut file = OpenOptions::new()
                .append(true)
                .open(&journal_path)
                .expect("Open for inject");
            // Write valid magic and claimed length of 50 bytes, but only write 10 trailing bytes
            file.write_all(&JOURNAL_MAGIC.to_be_bytes()).unwrap();
            file.write_all(&50u32.to_be_bytes()).unwrap();
            file.write_all(&[0xAA; 10]).unwrap(); // Truncated!
            file.sync_data().unwrap();
        }

        // Recovery must cleanly extract record 1 and ignore incomplete trailing bytes
        let recovered = DurableFileJournal::recover_from_journal(&journal_path).expect("Recover");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].lsn, 1);

        let _ = std::fs::remove_file(&journal_path);
    }
}
