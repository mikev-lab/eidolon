//! Vectorized batch UDP datagram I/O and zero-copy packet buffer pools.
//!
//! Provides pre-allocated datagram batches (`DatagramBatch`), fixed-size zero-allocation
//! buffer pools (`PacketBufferPool`), and high-precision rate monitors (`PacketRateMonitor`)
//! to eliminate context-switch and dynamic heap allocation overhead.

use std::net::SocketAddr;

use crate::protocol::MAX_PACKET_SIZE;

/// A single pre-allocated datagram slot in a batch buffer.
#[derive(Debug, Clone, Copy)]
pub struct DatagramSlot {
    /// Remote peer socket address.
    pub peer_addr: SocketAddr,
    /// Raw payload bytes.
    pub payload: [u8; MAX_PACKET_SIZE],
    /// Length of valid bytes in payload.
    pub len: usize,
}

impl Default for DatagramSlot {
    fn default() -> Self {
        Self {
            peer_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            payload: [0u8; MAX_PACKET_SIZE],
            len: 0,
        }
    }
}

impl DatagramSlot {
    /// Creates a slot initialized with a peer address and data slice.
    #[inline]
    pub fn new(peer_addr: SocketAddr, data: &[u8]) -> Option<Self> {
        if data.len() > MAX_PACKET_SIZE {
            return None;
        }
        let mut slot = Self {
            peer_addr,
            payload: [0u8; MAX_PACKET_SIZE],
            len: data.len(),
        };
        slot.payload[..data.len()].copy_from_slice(data);
        Some(slot)
    }

    /// Sets slot contents from peer address and slice.
    #[inline]
    pub fn set(&mut self, peer_addr: SocketAddr, data: &[u8]) -> bool {
        if data.len() > MAX_PACKET_SIZE {
            return false;
        }
        self.peer_addr = peer_addr;
        self.payload[..data.len()].copy_from_slice(data);
        self.len = data.len();
        true
    }

    /// Returns a slice of the valid payload bytes.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.payload[..self.len]
    }
}

/// Pre-allocated multi-packet batch container for vectorized UDP socket I/O.
#[derive(Debug)]
pub struct DatagramBatch<const BATCH_SIZE: usize> {
    slots: Box<[DatagramSlot; BATCH_SIZE]>,
    count: usize,
}

impl<const BATCH_SIZE: usize> Default for DatagramBatch<BATCH_SIZE> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const BATCH_SIZE: usize> DatagramBatch<BATCH_SIZE> {
    /// Constructs a new empty datagram batch with pre-allocated storage.
    pub fn new() -> Self {
        let v = vec![DatagramSlot::default(); BATCH_SIZE];
        let boxed_slice = v.into_boxed_slice();
        let slots: Box<[DatagramSlot; BATCH_SIZE]> = match boxed_slice.try_into() {
            Ok(arr) => arr,
            Err(_) => unreachable!("Vector length matches BATCH_SIZE exactly"),
        };
        Self { slots, count: 0 }
    }

    /// Clears all slots in the batch without deallocating.
    #[inline]
    pub fn clear(&mut self) {
        self.count = 0;
    }

    /// Returns the number of valid datagrams currently held in the batch.
    #[inline]
    pub const fn len(&self) -> usize {
        self.count
    }

    /// Returns true if the batch contains zero datagrams.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Returns true if the batch has reached maximum capacity.
    #[inline]
    pub const fn is_full(&self) -> bool {
        self.count >= BATCH_SIZE
    }

    /// Returns the maximum capacity of the batch.
    #[inline]
    pub const fn capacity(&self) -> usize {
        BATCH_SIZE
    }

    /// Appends a new datagram into the batch if capacity permits.
    #[inline]
    pub fn push(&mut self, peer_addr: SocketAddr, data: &[u8]) -> bool {
        if self.count >= BATCH_SIZE || data.len() > MAX_PACKET_SIZE {
            return false;
        }
        self.slots[self.count].set(peer_addr, data);
        self.count += 1;
        true
    }

    /// Pushes a prepared `DatagramSlot` into the batch if capacity permits.
    #[inline]
    pub fn push_slot(&mut self, slot: DatagramSlot) -> bool {
        if self.count >= BATCH_SIZE {
            return false;
        }
        self.slots[self.count] = slot;
        self.count += 1;
        true
    }

    /// Returns a reference to the slot at the specified index.
    #[inline]
    pub fn get(&self, index: usize) -> Option<&DatagramSlot> {
        if index < self.count {
            Some(&self.slots[index])
        } else {
            None
        }
    }

    /// Returns a mutable reference to the slot at the specified index.
    #[inline]
    pub fn get_mut(&mut self, index: usize) -> Option<&mut DatagramSlot> {
        if index < self.count {
            Some(&mut self.slots[index])
        } else {
            None
        }
    }

    /// Returns a slice of the active datagram slots.
    #[inline]
    pub fn as_slice(&self) -> &[DatagramSlot] {
        &self.slots[..self.count]
    }
}

/// High-throughput zero-copy packet buffer pool with pre-allocated flat storage.
///
/// Eliminates dynamic heap allocation during sustained packet reception or transmission bursts.
#[derive(Debug)]
pub struct PacketBufferPool<const POOL_CAP: usize, const BUFFER_SIZE: usize> {
    buffers: Box<[[u8; BUFFER_SIZE]]>,
    free_indices: Vec<usize>,
}

impl<const POOL_CAP: usize, const BUFFER_SIZE: usize> Default
    for PacketBufferPool<POOL_CAP, BUFFER_SIZE>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const POOL_CAP: usize, const BUFFER_SIZE: usize> PacketBufferPool<POOL_CAP, BUFFER_SIZE> {
    /// Constructs a new pre-allocated packet buffer pool.
    pub fn new() -> Self {
        let v = vec![[0u8; BUFFER_SIZE]; POOL_CAP];
        let mut free_indices = Vec::with_capacity(POOL_CAP);
        for i in (0..POOL_CAP).rev() {
            free_indices.push(i);
        }
        Self {
            buffers: v.into_boxed_slice(),
            free_indices,
        }
    }

    /// Leases a buffer index from the pool in O(1) time.
    #[inline]
    pub fn acquire(&mut self) -> Option<usize> {
        self.free_indices.pop()
    }

    /// Returns a previously leased buffer index back to the pool in O(1) time.
    #[inline]
    pub fn release(&mut self, index: usize) -> bool {
        if index < POOL_CAP && !self.free_indices.contains(&index) {
            self.free_indices.push(index);
            true
        } else {
            false
        }
    }

    /// Returns a reference to the buffer at the given index.
    #[inline]
    pub fn get(&self, index: usize) -> Option<&[u8; BUFFER_SIZE]> {
        self.buffers.get(index)
    }

    /// Returns a mutable reference to the buffer at the given index.
    #[inline]
    pub fn get_mut(&mut self, index: usize) -> Option<&mut [u8; BUFFER_SIZE]> {
        self.buffers.get_mut(index)
    }

    /// Returns the number of currently available buffers in the pool.
    #[inline]
    pub fn available_count(&self) -> usize {
        self.free_indices.len()
    }

    /// Returns total pool capacity.
    #[inline]
    pub const fn capacity(&self) -> usize {
        POOL_CAP
    }
}

/// High-precision rate monitor tracking ingress and egress packet counts and throughput.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketRateMonitor {
    total_packets: u64,
    total_bytes: u64,
    last_sample_tick: u64,
    packets_in_current_window: u64,
    bytes_in_current_window: u64,
    last_pps: u64,
    last_bps: u64,
    window_ticks: u64,
}

impl PacketRateMonitor {
    /// Creates a rate monitor with the specified window duration in simulation ticks (e.g. 20 ticks = 1 second at 20 Hz).
    pub fn new(window_ticks: u64) -> Self {
        Self {
            total_packets: 0,
            total_bytes: 0,
            last_sample_tick: 0,
            packets_in_current_window: 0,
            bytes_in_current_window: 0,
            last_pps: 0,
            last_bps: 0,
            window_ticks: window_ticks.max(1),
        }
    }

    /// Records newly ingested or egressed packet activity.
    #[inline]
    pub fn record(&mut self, packets: u64, bytes: u64) {
        self.total_packets += packets;
        self.total_bytes += bytes;
        self.packets_in_current_window += packets;
        self.bytes_in_current_window += bytes;
    }

    /// Samples the current tick, recalculating packets-per-second and bytes-per-second when the window boundary is crossed.
    ///
    /// Returns `(current_pps, current_bps)`.
    pub fn sample_tick(&mut self, current_tick: u64) -> (u64, u64) {
        if current_tick >= self.last_sample_tick + self.window_ticks {
            let elapsed_ticks = current_tick.saturating_sub(self.last_sample_tick).max(1);
            // Convert to per-second (assuming 20 Hz default cadence where 20 ticks = 1 second)
            let ticks_per_sec = 20u64;
            self.last_pps = (self.packets_in_current_window * ticks_per_sec) / elapsed_ticks;
            self.last_bps = (self.bytes_in_current_window * ticks_per_sec) / elapsed_ticks;

            self.packets_in_current_window = 0;
            self.bytes_in_current_window = 0;
            self.last_sample_tick = current_tick;
        }

        (self.last_pps, self.last_bps)
    }

    /// Returns the most recently computed packets per second.
    #[inline]
    pub const fn current_pps(&self) -> u64 {
        self.last_pps
    }

    /// Returns the most recently computed bytes per second.
    #[inline]
    pub const fn current_bps(&self) -> u64 {
        self.last_bps
    }

    /// Returns the cumulative lifetime packet count.
    #[inline]
    pub const fn total_packets(&self) -> u64 {
        self.total_packets
    }

    /// Returns the cumulative lifetime byte count.
    #[inline]
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_datagram_slot_creation_and_mutation() {
        let addr = "127.0.0.1:9000".parse().unwrap();
        let data = [1u8, 2, 3, 4, 5];

        let slot = DatagramSlot::new(addr, &data).unwrap();
        assert_eq!(slot.peer_addr, addr);
        assert_eq!(slot.len, 5);
        assert_eq!(slot.as_slice(), &[1, 2, 3, 4, 5]);

        let mut slot2 = DatagramSlot::default();
        assert_eq!(slot2.len, 0);
        assert!(slot2.set(addr, &[10, 20]));
        assert_eq!(slot2.len, 2);
        assert_eq!(slot2.as_slice(), &[10, 20]);

        // Oversized data rejected
        let oversized = [0u8; MAX_PACKET_SIZE + 1];
        assert!(DatagramSlot::new(addr, &oversized).is_none());
        assert!(!slot2.set(addr, &oversized));
    }

    #[test]
    fn test_datagram_batch_operations() {
        let mut batch = DatagramBatch::<16>::new();
        assert!(batch.is_empty());
        assert_eq!(batch.len(), 0);
        assert_eq!(batch.capacity(), 16);

        let addr = "10.0.0.1:8080".parse().unwrap();

        for i in 0..16 {
            let payload = [i as u8; 4];
            assert!(batch.push(addr, &payload));
        }

        assert!(batch.is_full());
        assert_eq!(batch.len(), 16);
        assert!(!batch.push(addr, &[1, 2, 3]));

        for i in 0..16 {
            let slot = batch.get(i).unwrap();
            assert_eq!(slot.as_slice(), &[i as u8; 4]);
        }

        assert!(batch.get(16).is_none());

        batch.clear();
        assert!(batch.is_empty());
        assert_eq!(batch.len(), 0);
    }

    #[test]
    fn test_packet_buffer_pool_lifecycle() {
        let mut pool = PacketBufferPool::<8, 256>::new();
        assert_eq!(pool.available_count(), 8);
        assert_eq!(pool.capacity(), 8);

        let mut leased = Vec::new();
        for _ in 0..8 {
            let idx = pool.acquire().expect("acquire index");
            leased.push(idx);
        }

        assert_eq!(pool.available_count(), 0);
        assert!(pool.acquire().is_none());

        for (i, &idx) in leased.iter().enumerate() {
            let buf = pool.get_mut(idx).unwrap();
            buf[0] = (i + 1) as u8;
        }

        for (i, &idx) in leased.iter().enumerate() {
            let buf = pool.get(idx).unwrap();
            assert_eq!(buf[0], (i + 1) as u8);
            assert!(pool.release(idx));
        }

        assert_eq!(pool.available_count(), 8);
        // Duplicate release rejected
        assert!(!pool.release(leased[0]));
    }

    #[test]
    fn test_packet_rate_monitor_sampling() {
        let mut monitor = PacketRateMonitor::new(20); // 1-second window at 20 Hz
        assert_eq!(monitor.total_packets(), 0);
        assert_eq!(monitor.total_bytes(), 0);

        // Record 100 packets totaling 12,000 bytes over ticks 0..20
        for _ in 0..20 {
            monitor.record(5, 600);
        }

        let (pps, bps) = monitor.sample_tick(20);
        assert_eq!(pps, 100);
        assert_eq!(bps, 12000);
        assert_eq!(monitor.total_packets(), 100);
        assert_eq!(monitor.total_bytes(), 12000);
        assert_eq!(monitor.current_pps(), 100);
        assert_eq!(monitor.current_bps(), 12000);
    }
}
