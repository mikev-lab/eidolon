//! Distributed quorum journal replication, multi-failure domain persistence, and disaster failover.
//!
//! Extends physical `fdatasync` commit semantics to multi-machine and multi-availability-zone (AZ)
//! deployments. When configured with `ReplicationMode::SynchronousQuorum`, transactions are synchronous
//! across a majority of storage nodes before client confirmation, delivering RPO = 0 across complete
//! host death or availability zone partition.

use std::collections::HashMap;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use crate::durable_journal::DurableFileJournal;
use crate::wal::WalRecord;

/// Replication mode governing durable transactional persistence across failure domains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplicationMode {
    /// Single-node local disk fsync only (RPO = 0 across local process crash).
    #[default]
    LocalOnly,
    /// Synchronous quorum requiring acknowledgment from W nodes out of N total replicas
    /// before confirming commit (RPO = 0 across complete machine or AZ disaster).
    SynchronousQuorum {
        /// Total storage nodes participating in the replication group.
        total_nodes: usize,
        /// Minimum acknowledgments required for commit confirmation.
        required_acks: usize,
    },
    /// Asynchronous mirroring: local fsync confirmation with best-effort background replication.
    AsynchronousMirror,
}

/// Manages synchronous and asynchronous replication of durable WAL journals across failure domains.
#[derive(Debug)]
pub struct ReplicatedJournalSink {
    primary: DurableFileJournal,
    primary_path: PathBuf,
    replicas: Vec<(PathBuf, DurableFileJournal)>,
    mode: ReplicationMode,
    records_replicated: u64,
    bytes_replicated: u64,
    quorum_successes: u64,
    quorum_failures: u64,
}

impl ReplicatedJournalSink {
    /// Constructs a new replicated journal sink with the specified primary journal and replication mode.
    pub fn new(primary_path: impl AsRef<Path>, mode: ReplicationMode) -> Result<Self, io::Error> {
        let path_buf = primary_path.as_ref().to_path_buf();
        let primary = DurableFileJournal::open(&path_buf)?;

        Ok(Self {
            primary,
            primary_path: path_buf,
            replicas: Vec::new(),
            mode,
            records_replicated: 0,
            bytes_replicated: 0,
            quorum_successes: 0,
            quorum_failures: 0,
        })
    }

    /// Adds a secondary replica journal destination to the replication group.
    pub fn add_replica(&mut self, replica_path: impl AsRef<Path>) -> Result<(), io::Error> {
        let path_buf = replica_path.as_ref().to_path_buf();
        let replica = DurableFileJournal::open(&path_buf)?;
        self.replicas.push((path_buf, replica));
        Ok(())
    }

    /// Returns the active replication mode.
    #[inline]
    pub fn mode(&self) -> ReplicationMode {
        self.mode
    }

    /// Sets the active replication mode.
    pub fn set_mode(&mut self, mode: ReplicationMode) {
        self.mode = mode;
    }

    /// Returns the path to the primary journal file.
    #[inline]
    pub fn primary_path(&self) -> &Path {
        &self.primary_path
    }

    /// Returns the number of secondary replicas currently registered.
    #[inline]
    pub fn replica_count(&self) -> usize {
        self.replicas.len()
    }

    /// Returns the total number of records successfully replicated.
    #[inline]
    pub fn records_replicated(&self) -> u64 {
        self.records_replicated
    }

    /// Returns the total bytes replicated.
    #[inline]
    pub fn bytes_replicated(&self) -> u64 {
        self.bytes_replicated
    }

    /// Appends a WAL record to the primary journal and synchronously replicates according to policy.
    ///
    /// Under `SynchronousQuorum`, blocks until `required_acks` storage nodes (including primary)
    /// guarantee physical persistence. If quorum cannot be satisfied, returns `ErrorKind::TimedOut`
    /// or `ErrorKind::BrokenPipe`, preventing client confirmation and maintaining RPO = 0.
    pub fn append_and_replicate(&mut self, record: &WalRecord) -> Result<usize, io::Error> {
        // Step 1: Write and sync to primary storage node
        let bytes_written = self.primary.append_and_sync(record)?;
        let mut acks = 1usize; // Primary acknowledgment secured

        match self.mode {
            ReplicationMode::LocalOnly => {
                self.records_replicated += 1;
                self.bytes_replicated += bytes_written as u64;
                Ok(bytes_written)
            }
            ReplicationMode::AsynchronousMirror => {
                for (_, replica) in self.replicas.iter_mut() {
                    let _ = replica.append_and_sync(record);
                }
                self.records_replicated += 1;
                self.bytes_replicated += bytes_written as u64;
                Ok(bytes_written)
            }
            ReplicationMode::SynchronousQuorum { required_acks, .. } => {
                for (_, replica) in self.replicas.iter_mut() {
                    if replica.append_and_sync(record).is_ok() {
                        acks += 1;
                    }
                }

                if acks >= required_acks {
                    self.quorum_successes += 1;
                    self.records_replicated += 1;
                    self.bytes_replicated += bytes_written as u64;
                    Ok(bytes_written)
                } else {
                    self.quorum_failures += 1;
                    Err(io::Error::new(
                        ErrorKind::TimedOut,
                        format!(
                            "Quorum write failed: required {required_acks} acks, achieved {acks}"
                        ),
                    ))
                }
            }
        }
    }

    /// Recovers all canonical committed WAL records across an ensemble of replica journal paths.
    ///
    /// Reconstructs the distributed consensus state: a transaction is considered durably committed
    /// if and only if it was persisted to at least `required_acks` replica journals with valid checksums.
    /// Incomplete transactions that failed to reach quorum are safely discarded without panics.
    pub fn recover_quorum<P: AsRef<Path>>(
        replica_paths: &[P],
        required_acks: usize,
    ) -> Result<Vec<WalRecord>, io::Error> {
        if replica_paths.is_empty() || required_acks == 0 {
            return Ok(Vec::new());
        }

        // Map: LSN -> (WalRecord, AckCount)
        let mut ack_map: HashMap<u64, (WalRecord, usize)> = HashMap::new();

        for path in replica_paths {
            if let Ok(records) = DurableFileJournal::recover_from_journal(path) {
                for rec in records {
                    let entry = ack_map.entry(rec.lsn).or_insert((rec, 0));
                    entry.1 += 1;
                }
            }
        }

        // Sort records by LSN and filter by quorum threshold
        let mut sorted_lsns: Vec<u64> = ack_map.keys().copied().collect();
        sorted_lsns.sort_unstable();

        let mut committed = Vec::new();
        let mut expected_lsn = 1u64;

        for lsn in sorted_lsns {
            if let Some((record, acks)) = ack_map.get(&lsn) {
                if *acks >= required_acks {
                    if lsn == expected_lsn {
                        committed.push(*record);
                        expected_lsn += 1;
                    } else if lsn < expected_lsn {
                        // Duplicate or already processed LSN
                        continue;
                    } else {
                        // Gap in sequence: stop recovery at clean boundary
                        break;
                    }
                }
            }
        }

        Ok(committed)
    }

    /// Promotes a surviving replica journal to assume primary authority following primary node loss.
    ///
    /// Copies or re-opens the replica journal as a standalone primary `DurableFileJournal`.
    pub fn promote_to_primary(
        replica_path: impl AsRef<Path>,
        new_primary_path: impl AsRef<Path>,
    ) -> Result<DurableFileJournal, io::Error> {
        let rep = replica_path.as_ref();
        let target = new_primary_path.as_ref();

        if rep != target {
            fs::copy(rep, target)?;
        }

        DurableFileJournal::open(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_record(lsn: u64, amount: u64) -> WalRecord {
        let mut payload = [0u8; 8];
        payload.copy_from_slice(&amount.to_be_bytes());
        WalRecord::new(lsn, 10, 5001, 0, 0x04, &payload).expect("Create test record")
    }

    #[test]
    fn test_replicated_journal_local_only() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("eidolon_repl_local_{}.wal", std::process::id()));
        let _ = fs::remove_file(&path);

        let mut sink =
            ReplicatedJournalSink::new(&path, ReplicationMode::LocalOnly).expect("Create sink");

        let rec = create_test_record(1, 100);
        let bytes = sink.append_and_replicate(&rec).expect("Append record");
        assert!(bytes > 0);
        assert_eq!(sink.records_replicated(), 1);

        let recovered = DurableFileJournal::recover_from_journal(&path).expect("Recover");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].lsn, 1);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_synchronous_quorum_success() {
        let dir = std::env::temp_dir();
        let p_path = dir.join(format!("eidolon_q_prim_{}.wal", std::process::id()));
        let r1_path = dir.join(format!("eidolon_q_r1_{}.wal", std::process::id()));
        let r2_path = dir.join(format!("eidolon_q_r2_{}.wal", std::process::id()));

        let _ = fs::remove_file(&p_path);
        let _ = fs::remove_file(&r1_path);
        let _ = fs::remove_file(&r2_path);

        // 3-node group: Primary + 2 Replicas, Quorum = 2 of 3
        let mut sink = ReplicatedJournalSink::new(
            &p_path,
            ReplicationMode::SynchronousQuorum {
                total_nodes: 3,
                required_acks: 2,
            },
        )
        .expect("Create sink");

        sink.add_replica(&r1_path).expect("Add r1");
        sink.add_replica(&r2_path).expect("Add r2");

        let rec1 = create_test_record(1, 250);
        let rec2 = create_test_record(2, 500);

        assert!(sink.append_and_replicate(&rec1).is_ok());
        assert!(sink.append_and_replicate(&rec2).is_ok());

        assert_eq!(sink.records_replicated(), 2);
        assert_eq!(sink.quorum_successes, 2);
        assert_eq!(sink.quorum_failures, 0);

        // Verify quorum recovery across all 3 nodes
        let recovered = ReplicatedJournalSink::recover_quorum(&[&p_path, &r1_path, &r2_path], 2)
            .expect("Quorum recovery");
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered[0].lsn, 1);
        assert_eq!(recovered[1].lsn, 2);

        let _ = fs::remove_file(&p_path);
        let _ = fs::remove_file(&r1_path);
        let _ = fs::remove_file(&r2_path);
    }

    #[test]
    fn test_synchronous_quorum_failure_when_under_threshold() {
        let dir = std::env::temp_dir();
        let p_path = dir.join(format!("eidolon_q_fail_prim_{}.wal", std::process::id()));
        let _ = fs::remove_file(&p_path);

        // Required 3 acks, but only primary exists (total 1 ack)
        let mut sink = ReplicatedJournalSink::new(
            &p_path,
            ReplicationMode::SynchronousQuorum {
                total_nodes: 3,
                required_acks: 3,
            },
        )
        .expect("Create sink");

        let rec = create_test_record(1, 100);
        let res = sink.append_and_replicate(&rec);
        assert!(res.is_err(), "Must fail when quorum cannot be satisfied");
        assert_eq!(sink.quorum_failures, 1);

        let _ = fs::remove_file(&p_path);
    }

    #[test]
    fn test_replica_promotion_and_failover() {
        let dir = std::env::temp_dir();
        let p_path = dir.join(format!("eidolon_prom_prim_{}.wal", std::process::id()));
        let r_path = dir.join(format!("eidolon_prom_repl_{}.wal", std::process::id()));
        let new_p_path = dir.join(format!("eidolon_prom_new_{}.wal", std::process::id()));

        let _ = fs::remove_file(&p_path);
        let _ = fs::remove_file(&r_path);
        let _ = fs::remove_file(&new_p_path);

        let mut sink = ReplicatedJournalSink::new(
            &p_path,
            ReplicationMode::SynchronousQuorum {
                total_nodes: 2,
                required_acks: 2,
            },
        )
        .expect("Create sink");

        sink.add_replica(&r_path).expect("Add replica");

        let rec = create_test_record(1, 1000);
        sink.append_and_replicate(&rec).expect("Append");

        // Primary node abruptly dies (simulated by deleting primary file)
        drop(sink);
        let _ = fs::remove_file(&p_path);

        // Standby promotes replica to new primary
        let mut promoted = ReplicatedJournalSink::promote_to_primary(&r_path, &new_p_path)
            .expect("Promote to primary");

        let next_rec = create_test_record(2, 2000);
        assert!(promoted.append_and_sync(&next_rec).is_ok());

        let final_recovered =
            DurableFileJournal::recover_from_journal(&new_p_path).expect("Recover promoted");
        assert_eq!(final_recovered.len(), 2);
        assert_eq!(final_recovered[0].lsn, 1);
        assert_eq!(final_recovered[1].lsn, 2);

        let _ = fs::remove_file(&r_path);
        let _ = fs::remove_file(&new_p_path);
    }
}
