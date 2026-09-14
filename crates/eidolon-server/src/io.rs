//! Decoupled asynchronous UDP network I/O worker.
//!
//! Executes high-throughput packet ingestion and egress batching on non-blocking
//! sockets without stalling the synchronous simulation loop.

use std::io::{self, ErrorKind};
use std::net::{SocketAddr, UdpSocket};

use eidolon_net::batch_io::DatagramBatch;
use eidolon_net::protocol::MAX_PACKET_SIZE;

use crate::queue::{NetworkPacket, SpscPacketQueue};

/// Asynchronous UDP network I/O worker operating in non-blocking mode.
#[derive(Debug)]
pub struct NetworkIoWorker {
    socket: UdpSocket,
}

impl NetworkIoWorker {
    /// Binds a new non-blocking UDP socket to the requested address.
    pub fn bind(addr: SocketAddr) -> io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        Ok(Self { socket })
    }

    /// Returns the local socket address this worker is bound to.
    #[inline]
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Drains incoming datagrams from the UDP socket into the ingress queue.
    ///
    /// Reads up to `max_packets` datagrams per call. Returns the number of packets enqueued.
    pub fn drain_ingress<const CAP: usize>(
        &self,
        ingress_queue: &SpscPacketQueue<CAP>,
        max_packets: usize,
    ) -> usize {
        let mut buffer = [0u8; MAX_PACKET_SIZE];
        let mut enqueued_count = 0;

        for _ in 0..max_packets {
            match self.socket.recv_from(&mut buffer) {
                Ok((len, peer_addr)) => {
                    if let Some(packet) = NetworkPacket::new(peer_addr, &buffer[..len]) {
                        if ingress_queue.try_push(packet) {
                            enqueued_count += 1;
                        } else {
                            // Ingress queue saturated: drop packet under backpressure
                            break;
                        }
                    }
                }
                Err(ref e)
                    if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
                {
                    // Socket drained
                    break;
                }
                Err(_) => {
                    // Ignorable socket network read error
                    break;
                }
            }
        }

        enqueued_count
    }

    /// Drains incoming datagrams from the UDP socket into the ingress queue, enforcing per-socket rate quotas.
    ///
    /// Evaluates rate limits using `SocketRatePolicer`. Datagrams exceeding rate quotas are dropped
    /// immediately before allocating space in the ingress queue.
    /// Returns `(enqueued_count, dropped_count)`.
    pub fn drain_ingress_policed<const CAP: usize>(
        &self,
        ingress_queue: &SpscPacketQueue<CAP>,
        policer: &mut eidolon_net::SocketRatePolicer,
        current_tick: u64,
        max_packets: usize,
    ) -> (usize, usize) {
        let mut buffer = [0u8; MAX_PACKET_SIZE];
        let mut enqueued_count = 0;
        let mut dropped_count = 0;

        for _ in 0..max_packets {
            match self.socket.recv_from(&mut buffer) {
                Ok((len, peer_addr)) => {
                    if policer.check_ingress(len, current_tick).is_err() {
                        dropped_count += 1;
                        continue;
                    }

                    if let Some(packet) = NetworkPacket::new(peer_addr, &buffer[..len]) {
                        if ingress_queue.try_push(packet) {
                            enqueued_count += 1;
                        } else {
                            // Queue saturated: drop packet under backpressure
                            dropped_count += 1;
                            break;
                        }
                    }
                }
                Err(ref e)
                    if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => {
                    break;
                }
            }
        }

        (enqueued_count, dropped_count)
    }

    /// Drains outgoing datagrams from the egress queue and transmits them over UDP.
    ///
    /// Sends up to `max_packets` datagrams per call. Returns the number of packets dispatched.
    pub fn flush_egress<const CAP: usize>(
        &self,
        egress_queue: &SpscPacketQueue<CAP>,
        max_packets: usize,
    ) -> usize {
        let mut sent_count = 0;

        for _ in 0..max_packets {
            if let Some(packet) = egress_queue.try_pop() {
                let payload = &packet.payload[..packet.len];
                match self.socket.send_to(payload, packet.peer_addr) {
                    Ok(_) => {
                        sent_count += 1;
                    }
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                        // Socket send buffer full: yield until next cycle
                        break;
                    }
                    Err(_) => {
                        // Drop failed send
                        break;
                    }
                }
            } else {
                // Egress queue empty
                break;
            }
        }

        sent_count
    }

    /// Drains incoming datagrams from the UDP socket in vectorized batches.
    ///
    /// Reads up to `BATCH_SIZE` datagrams per batch into the provided `DatagramBatch`
    /// and enqueues them into `ingress_queue` in a single lock acquisition.
    pub fn drain_ingress_batch<const BATCH_SIZE: usize, const CAP: usize>(
        &self,
        ingress_queue: &SpscPacketQueue<CAP>,
        batch: &mut DatagramBatch<BATCH_SIZE>,
    ) -> usize {
        let mut buffer = [0u8; MAX_PACKET_SIZE];
        let mut total_enqueued = 0;

        loop {
            batch.clear();
            while !batch.is_full() {
                match self.socket.recv_from(&mut buffer) {
                    Ok((len, peer_addr)) => {
                        if !batch.push(peer_addr, &buffer[..len]) {
                            break;
                        }
                    }
                    Err(ref e)
                        if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => {
                        break;
                    }
                }
            }

            if batch.is_empty() {
                break;
            }

            let batch_len = batch.len();
            let mut staging = [NetworkPacket::default(); BATCH_SIZE];
            for (i, item) in staging.iter_mut().enumerate().take(batch_len) {
                if let Some(slot) = batch.get(i) {
                    *item = (*slot).into();
                }
            }

            let enqueued = ingress_queue.try_push_batch(&staging[..batch_len]);
            total_enqueued += enqueued;

            if enqueued < batch_len {
                // Queue full under backpressure
                break;
            }
        }

        total_enqueued
    }

    /// Drains incoming datagrams with priority traffic shaping and adaptive backpressure shedding.
    ///
    /// When queue occupancy exceeds 80% capacity, drops unreliable sequenced packets,
    /// reserving queue space strictly for reliable messages and connection control.
    /// Returns `(enqueued_count, dropped_count)`.
    pub fn drain_ingress_batch_prioritized<const BATCH_SIZE: usize, const CAP: usize>(
        &self,
        ingress_queue: &SpscPacketQueue<CAP>,
        batch: &mut DatagramBatch<BATCH_SIZE>,
    ) -> (usize, usize) {
        let mut buffer = [0u8; MAX_PACKET_SIZE];
        let mut total_enqueued = 0;
        let mut total_shed = 0;
        let backpressure_threshold = (CAP * 80) / 100;

        loop {
            batch.clear();
            while !batch.is_full() {
                match self.socket.recv_from(&mut buffer) {
                    Ok((len, peer_addr)) => {
                        if !batch.push(peer_addr, &buffer[..len]) {
                            break;
                        }
                    }
                    Err(ref e)
                        if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => {
                        break;
                    }
                }
            }

            if batch.is_empty() {
                break;
            }

            let current_len = ingress_queue.len();
            let under_pressure = current_len >= backpressure_threshold;

            let mut staging = [NetworkPacket::default(); BATCH_SIZE];
            let mut stage_count = 0;

            for i in 0..batch.len() {
                if let Some(slot) = batch.get(i) {
                    if under_pressure {
                        let is_reliable = slot.len >= 4 && (slot.payload[3] & 0x01) != 0;
                        if !is_reliable {
                            total_shed += 1;
                            continue;
                        }
                    }
                    staging[stage_count] = (*slot).into();
                    stage_count += 1;
                }
            }

            if stage_count > 0 {
                let enqueued = ingress_queue.try_push_batch(&staging[..stage_count]);
                total_enqueued += enqueued;
                total_shed += stage_count.saturating_sub(enqueued);
            }

            if batch.len() < BATCH_SIZE {
                break;
            }
        }

        (total_enqueued, total_shed)
    }

    /// Flushes outgoing datagrams from the egress queue in vectorized batches.
    ///
    /// Pulls up to `BATCH_SIZE` packets in a single lock acquisition and transmits them over UDP.
    /// Returns the total number of packets dispatched.
    pub fn flush_egress_batch<const BATCH_SIZE: usize, const CAP: usize>(
        &self,
        egress_queue: &SpscPacketQueue<CAP>,
    ) -> usize {
        let mut staging = [NetworkPacket::default(); BATCH_SIZE];
        let mut total_sent = 0;

        loop {
            let popped = egress_queue.try_pop_batch(&mut staging);
            if popped == 0 {
                break;
            }

            for pkt in &staging[..popped] {
                let payload = &pkt.payload[..pkt.len];
                match self.socket.send_to(payload, pkt.peer_addr) {
                    Ok(_) => {
                        total_sent += 1;
                    }
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                        return total_sent;
                    }
                    Err(_) => {
                        // Drop socket transmission error packet
                    }
                }
            }

            if popped < BATCH_SIZE {
                break;
            }
        }

        total_sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_network_io_worker_loopback_drain() {
        let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let worker = NetworkIoWorker::bind(server_addr).expect("bind worker");
        let bound_addr = worker.local_addr().expect("local addr");

        // Client socket sends a test packet
        let client = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .expect("bind client");
        client
            .send_to(b"hello eidolon", bound_addr)
            .expect("send to server");

        let ingress_queue = SpscPacketQueue::<16>::new();

        // Drain ingress with bounded retry to accommodate OS packet delivery
        let mut count = 0;
        for _ in 0..50 {
            count += worker.drain_ingress(&ingress_queue, 10);
            if count >= 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(count, 1);
        assert_eq!(ingress_queue.len(), 1);

        let packet = ingress_queue.try_pop().expect("pop packet");
        assert_eq!(&packet.payload[..packet.len], b"hello eidolon");
    }

    #[test]
    fn test_network_io_worker_batch_drain_and_flush() {
        let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let server_worker = NetworkIoWorker::bind(server_addr).expect("bind server");
        let server_bound = server_worker.local_addr().expect("server addr");

        let client_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let client_worker = NetworkIoWorker::bind(client_addr).expect("bind client");
        let client_bound = client_worker.local_addr().expect("client addr");

        // Client sends 8 packets to server
        for i in 0..8 {
            let msg = [i as u8; 4];
            client_worker
                .socket
                .send_to(&msg, server_bound)
                .expect("client send");
        }

        let ingress_queue = SpscPacketQueue::<32>::new();
        let mut batch = DatagramBatch::<16>::new();

        let mut drained = 0;
        for _ in 0..50 {
            drained += server_worker.drain_ingress_batch(&ingress_queue, &mut batch);
            if drained >= 8 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(drained, 8);
        assert_eq!(ingress_queue.len(), 8);

        // Server flushes egress packets back to client
        let egress_queue = SpscPacketQueue::<32>::new();
        for i in 0..8 {
            let msg = [i as u8 + 10; 4];
            let pkt = NetworkPacket::new(client_bound, &msg).expect("pkt");
            egress_queue.try_push(pkt);
        }

        let sent = server_worker.flush_egress_batch::<16, 32>(&egress_queue);
        assert_eq!(sent, 8);
        assert_eq!(egress_queue.len(), 0);

        // Client drains ingress batch
        let client_ingress = SpscPacketQueue::<32>::new();
        let mut client_batch = DatagramBatch::<16>::new();
        let mut client_drained = 0;
        for _ in 0..50 {
            client_drained += client_worker.drain_ingress_batch(&client_ingress, &mut client_batch);
            if client_drained >= 8 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(client_drained, 8);
    }

    #[test]
    fn test_network_io_worker_prioritized_shedding() {
        let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let worker = NetworkIoWorker::bind(server_addr).expect("bind server");
        let bound_addr = worker.local_addr().expect("local addr");

        let client = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .expect("bind client");

        // Queue capacity 10, pre-fill with 8 dummy packets (80% capacity = threshold)
        let ingress_queue = SpscPacketQueue::<10>::new();
        for _ in 0..8 {
            ingress_queue.try_push(
                NetworkPacket::new(bound_addr, &[0x45, 0x49, 0, 0, 1]).expect("dummy pkt"),
            );
        }
        assert_eq!(ingress_queue.len(), 8);

        // Send 2 unreliable packets (payload[3] & 0x01 == 0) and 2 reliable packets (payload[3] & 0x01 != 0)
        let unreliable_pkt = [0x45, 0x49, 0, 0x00, 10];
        let reliable_pkt = [0x45, 0x49, 0, 0x01, 20];

        client
            .send_to(&unreliable_pkt, bound_addr)
            .expect("send unreliable 1");
        client
            .send_to(&unreliable_pkt, bound_addr)
            .expect("send unreliable 2");
        client
            .send_to(&reliable_pkt, bound_addr)
            .expect("send reliable 1");
        client
            .send_to(&reliable_pkt, bound_addr)
            .expect("send reliable 2");

        let mut batch = DatagramBatch::<16>::new();
        let mut total_enq = 0;
        let mut total_shed = 0;

        for _ in 0..50 {
            let (enq, shed) = worker.drain_ingress_batch_prioritized(&ingress_queue, &mut batch);
            total_enq += enq;
            total_shed += shed;
            if total_enq + total_shed >= 4 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        // Unreliable packets were shed due to backpressure (>80%), reliable packets were admitted
        assert_eq!(total_enq, 2);
        assert_eq!(total_shed, 2);
        assert_eq!(ingress_queue.len(), 10);
    }
}
