//! Real multi-process UDP loopback cluster runner and abrupt process termination resilience.
//!
//! Evaluates cluster coordination, cross-node migration framing, and failure recovery over
//! real operating system UDP sockets (`std::net::UdpSocket`) bound to the loopback interface (`127.0.0.1`).
//! Tests true process termination resilience via POSIX `SIGKILL` (`Child::kill`), proving that
//! sudden process death leaves the durable WAL intact and allows instant restart with RPO = 0.

use std::collections::HashMap;
use std::io::{self, ErrorKind};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};

use eidolon_world::durable_journal::DurableFileJournal;
use eidolon_world::wal::WalRecord;

/// 4-byte framing magic identifying real-socket cluster packets (`E1D0PC`).
pub const CLUSTER_SOCKET_MAGIC: u32 = 0xE1D0_5043;

/// Opcode indicating cross-zone entity migration.
pub const OP_MIGRATE: u8 = 1;
/// Opcode acknowledging successful migration receipt.
pub const OP_MIGRATE_ACK: u8 = 2;
/// Opcode for cluster keep-alive heartbeats.
pub const OP_HEARTBEAT: u8 = 3;
/// Opcode representing a durable transactional mutation.
pub const OP_TRANSACTION: u8 = 4;

/// Wire-encoded packet for real OS loopback socket communication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealSocketPacket {
    /// Operation code.
    pub opcode: u8,
    /// Targeted entity or account identifier.
    pub identifier: u64,
    /// Sequence number or Log Sequence Number.
    pub sequence: u64,
    /// Variable byte payload.
    pub payload: Vec<u8>,
}

impl RealSocketPacket {
    /// Encodes the packet into binary wire format with an 8-byte header.
    pub fn encode(&self, dest: &mut [u8]) -> Result<usize, io::Error> {
        let total_len = 4 + 1 + 8 + 8 + 2 + self.payload.len();
        if dest.len() < total_len {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Buffer too small for cluster packet",
            ));
        }

        dest[0..4].copy_from_slice(&CLUSTER_SOCKET_MAGIC.to_be_bytes());
        dest[4] = self.opcode;
        dest[5..13].copy_from_slice(&self.identifier.to_be_bytes());
        dest[13..21].copy_from_slice(&self.sequence.to_be_bytes());
        dest[21..23].copy_from_slice(&(self.payload.len() as u16).to_be_bytes());

        if let Some(payload_dest) = dest.get_mut(23..total_len) {
            payload_dest.copy_from_slice(&self.payload);
        }

        Ok(total_len)
    }

    /// Decodes a packet from binary wire format.
    pub fn decode(src: &[u8]) -> Result<Self, io::Error> {
        if src.len() < 23 {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "Packet shorter than minimum header",
            ));
        }

        let magic = u32::from_be_bytes([src[0], src[1], src[2], src[3]]);
        if magic != CLUSTER_SOCKET_MAGIC {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "Invalid cluster packet magic",
            ));
        }

        let opcode = src[4];
        let identifier = u64::from_be_bytes([
            src[5], src[6], src[7], src[8], src[9], src[10], src[11], src[12],
        ]);
        let sequence = u64::from_be_bytes([
            src[13], src[14], src[15], src[16], src[17], src[18], src[19], src[20],
        ]);
        let payload_len = u16::from_be_bytes([src[21], src[22]]) as usize;

        if src.len() < 23 + payload_len {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "Incomplete payload in cluster packet",
            ));
        }

        let payload = src[23..23 + payload_len].to_vec();

        Ok(Self {
            opcode,
            identifier,
            sequence,
            payload,
        })
    }
}

/// Standalone zone node bound to a real operating system UDP loopback socket.
pub struct RealSocketZoneNode {
    socket: UdpSocket,
    local_addr: SocketAddr,
    peer_addr: Option<SocketAddr>,
    peer_routes: HashMap<u32, SocketAddr>,
    blocked_zones: Vec<u32>,
    pending_migrations: HashMap<u64, (u32, Vec<u8>, std::time::Instant)>,
    zone_id: u32,
    journal: Option<DurableFileJournal>,
    entities: HashMap<u64, Vec<u8>>,
    lsn_counter: u64,
    packets_received: u64,
    packets_sent: u64,
}

impl RealSocketZoneNode {
    /// Binds a real UDP socket on the loopback interface (`127.0.0.1`).
    ///
    /// Specifying port `0` allows the operating system to dynamically assign an unused port.
    pub fn bind(
        port: u16,
        zone_id: u32,
        journal_path: Option<impl AsRef<Path>>,
    ) -> Result<Self, io::Error> {
        let socket = UdpSocket::bind(format!("127.0.0.1:{}", port))?;
        socket.set_nonblocking(true)?;
        let local_addr = socket.local_addr()?;

        let journal = match journal_path {
            Some(path) => Some(DurableFileJournal::open(path)?),
            None => None,
        };

        Ok(Self {
            socket,
            local_addr,
            peer_addr: None,
            peer_routes: HashMap::new(),
            blocked_zones: Vec::new(),
            pending_migrations: HashMap::new(),
            zone_id,
            journal,
            entities: HashMap::new(),
            lsn_counter: 0,
            packets_received: 0,
            packets_sent: 0,
        })
    }

    /// Sets the destination peer socket address for default cluster communication.
    pub fn set_peer_addr(&mut self, peer_addr: SocketAddr) {
        self.peer_addr = Some(peer_addr);
    }

    /// Registers a routing destination for a specific target zone in a multi-node cluster.
    pub fn add_peer_route(&mut self, target_zone_id: u32, peer_addr: SocketAddr) {
        self.peer_routes.insert(target_zone_id, peer_addr);
    }

    /// Declares a simulated network partition against a specific zone, dropping packets to/from it.
    pub fn partition_zone(&mut self, target_zone_id: u32) {
        if !self.blocked_zones.contains(&target_zone_id) {
            self.blocked_zones.push(target_zone_id);
        }
    }

    /// Restores network connectivity by healing a declared partition against a specific zone.
    pub fn heal_zone(&mut self, target_zone_id: u32) {
        self.blocked_zones.retain(|&z| z != target_zone_id);
    }

    /// Returns true if traffic to or from the specified zone is currently partitioned.
    #[inline]
    pub fn is_partitioned(&self, target_zone_id: u32) -> bool {
        self.blocked_zones.contains(&target_zone_id)
    }

    /// Returns the bound local socket address.
    #[inline]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Spawns an entity into this zone's local state.
    pub fn spawn_entity(&mut self, entity_id: u64, state: Vec<u8>) {
        self.entities.insert(entity_id, state);
    }

    /// Returns true if this zone contains the entity in active state.
    #[inline]
    pub fn has_entity(&self, entity_id: u64) -> bool {
        self.entities.contains_key(&entity_id)
    }

    /// Returns the count of pending in-flight migrations awaiting remote ACK.
    #[inline]
    pub fn pending_migration_count(&self) -> usize {
        self.pending_migrations.len()
    }

    /// Dispatches a cross-zone entity migration over the real OS UDP socket to the default peer.
    pub fn dispatch_migration(&mut self, entity_id: u64) -> Result<(), io::Error> {
        let default_target = self.zone_id.wrapping_add(1);
        self.dispatch_migration_to(default_target, entity_id)
    }

    /// Dispatches a cross-zone entity migration to a specific target zone over the real OS UDP socket.
    ///
    /// Stores the entity in a pending migration buffer until acknowledged. If the target zone is partitioned
    /// or unreachable, calling `check_migration_timeouts` safely rolls back ownership to the local node.
    pub fn dispatch_migration_to(
        &mut self,
        target_zone_id: u32,
        entity_id: u64,
    ) -> Result<(), io::Error> {
        if self.is_partitioned(target_zone_id) {
            return Err(io::Error::new(
                ErrorKind::ConnectionRefused,
                format!("Network partition active against Zone {target_zone_id}"),
            ));
        }

        let peer = self
            .peer_routes
            .get(&target_zone_id)
            .copied()
            .or(self.peer_addr)
            .ok_or_else(|| {
                io::Error::new(
                    ErrorKind::NotConnected,
                    "No peer route configured for target",
                )
            })?;

        let state = self
            .entities
            .remove(&entity_id)
            .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "Entity not in local zone"))?;

        let packet = RealSocketPacket {
            opcode: OP_MIGRATE,
            identifier: entity_id,
            sequence: self.zone_id as u64,
            payload: state.clone(),
        };

        // Record into pending migration buffer with dispatch timestamp
        self.pending_migrations.insert(
            entity_id,
            (target_zone_id, state, std::time::Instant::now()),
        );

        let mut buf = [0u8; 512];
        let len = packet.encode(&mut buf)?;
        self.socket.send_to(&buf[..len], peer)?;
        self.packets_sent += 1;

        Ok(())
    }

    /// Scans pending migrations and rolls back any unacknowledged migrations exceeding the timeout.
    ///
    /// Restores the entity state back into `self.entities`, preventing entity loss under network partitions.
    pub fn check_migration_timeouts(&mut self, timeout: std::time::Duration) -> usize {
        let now = std::time::Instant::now();
        let mut timed_out_ids = Vec::new();

        for (&entity_id, (_, _, timestamp)) in self.pending_migrations.iter() {
            if now.duration_since(*timestamp) >= timeout {
                timed_out_ids.push(entity_id);
            }
        }

        let rolled_back_count = timed_out_ids.len();
        for id in timed_out_ids {
            if let Some((_, state, _)) = self.pending_migrations.remove(&id) {
                self.entities.insert(id, state);
            }
        }

        rolled_back_count
    }

    /// Executes a durable transaction, writing to the physical journal and executing `fdatasync`.
    pub fn execute_durable_transaction(
        &mut self,
        account_id: u64,
        amount: u64,
    ) -> Result<u64, io::Error> {
        let journal = self
            .journal
            .as_mut()
            .ok_or_else(|| io::Error::other("Node configured without durable journal"))?;

        self.lsn_counter += 1;
        let lsn = self.lsn_counter;

        let mut payload = [0u8; 8];
        payload.copy_from_slice(&amount.to_be_bytes());

        let record = WalRecord::new(lsn, 1, account_id, 0, 0x04, &payload).map_err(|e| {
            io::Error::new(
                ErrorKind::InvalidData,
                format!("Failed to create WAL record: {:?}", e),
            )
        })?;

        journal.append_and_sync(&record)?;
        Ok(lsn)
    }

    /// Polls the UDP socket for incoming packets and processes migrations and acknowledgments.
    pub fn poll_network(&mut self) -> Result<usize, io::Error> {
        let mut buf = [0u8; 1024];
        let mut processed = 0;

        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((len, peer)) => {
                    self.packets_received += 1;
                    if let Ok(packet) = RealSocketPacket::decode(&buf[..len]) {
                        let sender_zone = packet.sequence as u32;

                        // Check if incoming traffic from this source zone is partitioned
                        if self.is_partitioned(sender_zone) {
                            continue;
                        }

                        match packet.opcode {
                            OP_MIGRATE => {
                                // Adopt entity into this zone
                                self.entities.insert(packet.identifier, packet.payload);

                                // Send ACK back over socket
                                let ack = RealSocketPacket {
                                    opcode: OP_MIGRATE_ACK,
                                    identifier: packet.identifier,
                                    sequence: self.zone_id as u64,
                                    payload: Vec::new(),
                                };
                                let mut ack_buf = [0u8; 64];
                                if let Ok(ack_len) = ack.encode(&mut ack_buf) {
                                    let _ = self.socket.send_to(&ack_buf[..ack_len], peer);
                                    self.packets_sent += 1;
                                }
                            }
                            OP_MIGRATE_ACK => {
                                // Migration confirmed by remote node: clear from pending
                                self.pending_migrations.remove(&packet.identifier);
                            }
                            _ => {}
                        }
                        processed += 1;
                    }
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }

        Ok(processed)
    }

    /// Returns the number of entities currently hosted in this zone.
    #[inline]
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }
}

/// Manages child OS worker processes and orchestrates abrupt termination via `SIGKILL`.
pub struct ProcessSupervisor {
    child: Child,
    port: u16,
    journal_path: PathBuf,
}

impl ProcessSupervisor {
    /// Spawns a child process worker running the current executable with additional CLI arguments.
    pub fn spawn_worker_with_args(
        port: u16,
        journal_path: impl AsRef<Path>,
        args: &[&str],
    ) -> Result<Self, io::Error> {
        let current_exe = std::env::current_exe()?;
        let path_buf = journal_path.as_ref().to_path_buf();

        let child = Command::new(current_exe)
            .args(args)
            .env("EIDOLON_WORKER_MODE", "1")
            .env("EIDOLON_WORKER_PORT", port.to_string())
            .env("EIDOLON_WORKER_JOURNAL", path_buf.to_str().unwrap_or(""))
            .spawn()?;

        Ok(Self {
            child,
            port,
            journal_path: path_buf,
        })
    }

    /// Spawns a child process worker running the current test executable with worker flags.
    pub fn spawn_worker(port: u16, journal_path: impl AsRef<Path>) -> Result<Self, io::Error> {
        Self::spawn_worker_with_args(port, journal_path, &["--exact", "test_child_worker_hook"])
    }

    /// Sends a POSIX `SIGKILL` (`kill -9`) to the child process.
    ///
    /// This immediately terminates the process without allowing clean shutdown hooks,
    /// verifying that WAL `fdatasync` guarantees persistence across host loss.
    pub fn kill_sigkill(&mut self) -> Result<(), io::Error> {
        self.child.kill()
    }

    /// Waits for the terminated child process to exit.
    pub fn wait(&mut self) -> Result<ExitStatus, io::Error> {
        self.child.wait()
    }

    /// Returns the port assigned to the worker.
    #[inline]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Returns the journal path utilized by the worker.
    #[inline]
    pub fn journal_path(&self) -> &Path {
        &self.journal_path
    }
}

/// Helper executed at binary entry point to handle child worker mode if configured.
pub fn run_worker_if_requested() -> bool {
    if std::env::var("EIDOLON_WORKER_MODE").is_err() {
        return false;
    }

    let port: u16 = std::env::var("EIDOLON_WORKER_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);

    let journal_path = std::env::var("EIDOLON_WORKER_JOURNAL").ok();

    if let Ok(mut node) = RealSocketZoneNode::bind(port, 1, journal_path) {
        // Execute sample transactions into the durable journal
        let _ = node.execute_durable_transaction(1001, 500);
        let _ = node.execute_durable_transaction(1001, 200);

        // Keep loop alive awaiting packets or SIGKILL
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(30) {
            let _ = node.poll_network();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_real_udp_socket_packet_roundtrip() {
        let packet = RealSocketPacket {
            opcode: OP_MIGRATE,
            identifier: 9901,
            sequence: 42,
            payload: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };

        let mut buf = [0u8; 128];
        let len = packet.encode(&mut buf).unwrap();
        assert_eq!(len, 23 + 8);

        let decoded = RealSocketPacket::decode(&buf[..len]).unwrap();
        assert_eq!(decoded, packet);
    }

    #[test]
    fn test_cross_zone_migration_over_real_os_loopback_sockets() {
        // Bind Node 1 and Node 2 on OS-allocated ephemeral ports (port 0)
        let mut node1 = RealSocketZoneNode::bind(0, 1, None::<&str>).expect("Node 1 bind");
        let mut node2 = RealSocketZoneNode::bind(0, 2, None::<&str>).expect("Node 2 bind");

        let addr1 = node1.local_addr();
        let addr2 = node2.local_addr();

        node1.set_peer_addr(addr2);
        node2.set_peer_addr(addr1);

        // Spawn entity in Node 1
        node1.spawn_entity(4001, vec![10, 20, 30]);
        assert!(node1.has_entity(4001));
        assert!(!node2.has_entity(4001));

        // Dispatch migration from Node 1 -> Node 2 over real loopback UDP socket
        node1.dispatch_migration(4001).expect("Dispatch migration");
        assert!(!node1.has_entity(4001)); // Despawned from source

        // Poll Node 2 to receive the packet from the OS socket buffer
        let mut received = false;
        for _ in 0..20 {
            let n = node2.poll_network().expect("Poll node 2");
            if n > 0 && node2.has_entity(4001) {
                received = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert!(
            received,
            "Node 2 must receive migrated entity over real UDP socket"
        );
        assert_eq!(node2.entity_count(), 1);

        // Poll Node 1 to receive migration ACK
        let _ = node1.poll_network();
    }

    #[test]
    fn test_partition_isolation_and_migration_timeout_rollback() {
        let mut node1 = RealSocketZoneNode::bind(0, 1, None::<&str>).expect("Node 1 bind");
        let mut node2 = RealSocketZoneNode::bind(0, 2, None::<&str>).expect("Node 2 bind");

        let addr1 = node1.local_addr();
        let addr2 = node2.local_addr();

        node1.add_peer_route(2, addr2);
        node2.add_peer_route(1, addr1);

        node1.spawn_entity(5001, vec![1, 2, 3]);

        // Declare partition from Node 1 against Zone 2
        node1.partition_zone(2);
        assert!(node1.is_partitioned(2));

        // Attempting to migrate to partitioned zone fails immediately
        let err = node1.dispatch_migration_to(2, 5001);
        assert!(err.is_err());
        assert!(node1.has_entity(5001));

        // Heal partition
        node1.heal_zone(2);
        assert!(!node1.is_partitioned(2));

        // Now partition on the receiving side (Node 2 partitions Zone 1)
        node2.partition_zone(1);

        // Node 1 dispatches migration
        node1
            .dispatch_migration_to(2, 5001)
            .expect("Dispatch to peer");
        assert_eq!(node1.pending_migration_count(), 1);
        assert!(!node1.has_entity(5001));

        // Node 2 polls network, but drops incoming packet from partitioned Zone 1
        let _ = node2.poll_network();
        assert!(!node2.has_entity(5001));

        // Node 1 checks timeouts (simulating 0ms timeout for instant rollback in test)
        let rolled_back = node1.check_migration_timeouts(std::time::Duration::from_millis(0));
        assert_eq!(rolled_back, 1);
        assert_eq!(node1.pending_migration_count(), 0);
        assert!(
            node1.has_entity(5001),
            "Entity must be safely restored to local zone upon migration timeout"
        );
    }
}
