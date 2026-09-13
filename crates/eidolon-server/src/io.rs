//! Decoupled asynchronous UDP network I/O worker.
//!
//! Executes high-throughput packet ingestion and egress batching on non-blocking
//! sockets without stalling the synchronous simulation loop.

use std::io::{self, ErrorKind};
use std::net::{SocketAddr, UdpSocket};

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
}
