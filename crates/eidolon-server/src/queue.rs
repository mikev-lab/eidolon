//! Bounded SPSC cross-thread packet queue for zero-allocation I/O handoffs.
//!
//! Bridges asynchronous UDP network worker threads and synchronous fixed-interval
//! simulation tick loops with hard memory limits and zero dynamic heap allocations.

use std::net::SocketAddr;
use std::sync::Mutex;

use eidolon_net::protocol::MAX_PACKET_SIZE;

/// Self-contained network packet with pre-allocated inline payload buffer.
#[derive(Debug, Clone, Copy)]
pub struct NetworkPacket {
    /// Remote client peer socket address.
    pub peer_addr: SocketAddr,
    /// Raw datagram payload data.
    pub payload: [u8; MAX_PACKET_SIZE],
    /// Length of valid data in the payload buffer.
    pub len: usize,
}

impl NetworkPacket {
    /// Creates a new network packet copying data into the fixed-size buffer.
    pub fn new(peer_addr: SocketAddr, data: &[u8]) -> Option<Self> {
        if data.len() > MAX_PACKET_SIZE {
            return None;
        }

        let mut payload = [0u8; MAX_PACKET_SIZE];
        payload.get_mut(..data.len())?.copy_from_slice(data);

        Some(Self {
            peer_addr,
            payload,
            len: data.len(),
        })
    }
}

#[derive(Debug)]
struct QueueState<const CAP: usize> {
    slots: Box<[Option<NetworkPacket>]>,
    head: usize,
    tail: usize,
    len: usize,
}

/// Thread-safe bounded packet queue supporting non-blocking push and pop operations.
#[derive(Debug)]
pub struct SpscPacketQueue<const CAP: usize> {
    state: Mutex<QueueState<CAP>>,
}

impl<const CAP: usize> SpscPacketQueue<CAP> {
    /// Constructs a new empty bounded queue with pre-allocated slots.
    pub fn new() -> Self {
        let mut slots = Vec::with_capacity(CAP);
        slots.resize_with(CAP, || None);
        Self {
            state: Mutex::new(QueueState {
                slots: slots.into_boxed_slice(),
                head: 0,
                tail: 0,
                len: 0,
            }),
        }
    }

    /// Pushes a packet into the queue if capacity is available.
    ///
    /// Returns true if successfully enqueued, or false if the queue is full.
    pub fn try_push(&self, packet: NetworkPacket) -> bool {
        let mut state = match self.state.lock() {
            Ok(guard) => guard,
            Err(_) => return false,
        };

        if state.len >= CAP {
            return false;
        }

        let tail = state.tail;
        if let Some(slot) = state.slots.get_mut(tail) {
            *slot = Some(packet);
            state.tail = (tail + 1) % CAP;
            state.len += 1;
            true
        } else {
            false
        }
    }

    /// Removes and returns the oldest packet from the queue, or None if empty.
    pub fn try_pop(&self) -> Option<NetworkPacket> {
        let mut state = self.state.lock().ok()?;
        if state.len == 0 {
            return None;
        }

        let head = state.head;
        let packet = state.slots.get_mut(head)?.take();
        state.head = (head + 1) % CAP;
        state.len = state.len.saturating_sub(1);
        packet
    }

    /// Drains available packets into the destination slice, returning the number drained.
    pub fn drain_into(&self, dest: &mut [Option<NetworkPacket>]) -> usize {
        let mut state = match self.state.lock() {
            Ok(guard) => guard,
            Err(_) => return 0,
        };

        let count = dest.len().min(state.len);
        for slot in dest.iter_mut().take(count) {
            let head = state.head;
            *slot = state.slots.get_mut(head).and_then(|s| s.take());
            state.head = (head + 1) % CAP;
        }
        state.len = state.len.saturating_sub(count);
        count
    }

    /// Returns the number of packets currently buffered in the queue.
    pub fn len(&self) -> usize {
        self.state.lock().map(|s| s.len).unwrap_or(0)
    }

    /// Returns true if the queue contains no packets.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns true if the queue is at maximum capacity.
    pub fn is_full(&self) -> bool {
        self.len() >= CAP
    }
}

impl<const CAP: usize> Default for SpscPacketQueue<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_spsc_packet_queue_fifo_flow() {
        let queue = SpscPacketQueue::<4>::new();
        assert!(queue.is_empty());

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8888);
        let p1 = NetworkPacket::new(addr, b"packet 1").expect("p1");
        let p2 = NetworkPacket::new(addr, b"packet 2").expect("p2");
        let p3 = NetworkPacket::new(addr, b"packet 3").expect("p3");
        let p4 = NetworkPacket::new(addr, b"packet 4").expect("p4");
        let p5 = NetworkPacket::new(addr, b"packet 5").expect("p5");

        assert!(queue.try_push(p1));
        assert!(queue.try_push(p2));
        assert!(queue.try_push(p3));
        assert!(queue.try_push(p4));
        // Queue full
        assert!(!queue.try_push(p5));
        assert!(queue.is_full());

        let popped1 = queue.try_pop().expect("pop 1");
        assert_eq!(&popped1.payload[..popped1.len], b"packet 1");

        let popped2 = queue.try_pop().expect("pop 2");
        assert_eq!(&popped2.payload[..popped2.len], b"packet 2");

        assert_eq!(queue.len(), 2);
    }
}
