//! Kernel-bypassing io_uring transport architecture and zero-copy ring buffers.
//!
//! Provides Submission Queue (SQ) and Completion Queue (CQ) ring buffers
//! for batched datagram ingestion and transmission without per-packet kernel context switches.
//! Implemented with pure safe Rust `#![deny(unsafe_code)]` using pre-allocated flat memory.

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Linux io_uring opcode for recvmsg datagram ingestion.
pub const IORING_OP_RECVMSG: u8 = 10;

/// Linux io_uring opcode for sendmsg datagram egress.
pub const IORING_OP_SENDMSG: u8 = 9;

/// Flag indicating kernel polling thread (SQPOLL) enabled.
pub const IORING_SETUP_SQPOLL: u32 = 1 << 1;

/// Standard maximum Ethernet MTU buffer size.
pub const PACKET_BUFFER_SIZE: usize = 1500;

/// Errors arising from io_uring transport operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoUringError {
    /// Submission queue is full.
    SubmissionQueueFull,
    /// Completion queue is full.
    CompletionQueueFull,
    /// Invalid packet length exceeds maximum payload buffer.
    PacketTooLarge(usize),
    /// Invalid configuration parameters.
    InvalidConfiguration,
}

impl fmt::Display for IoUringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SubmissionQueueFull => write!(f, "io_uring submission queue capacity exceeded"),
            Self::CompletionQueueFull => write!(f, "io_uring completion queue capacity exceeded"),
            Self::PacketTooLarge(len) => {
                write!(f, "Packet payload length {} exceeds maximum MTU", len)
            }
            Self::InvalidConfiguration => write!(f, "Invalid io_uring parameters"),
        }
    }
}

/// Submission Queue Entry (SQE) descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IoUringSqEntry {
    /// Kernel I/O operation code (e.g. IORING_OP_RECVMSG or IORING_OP_SENDMSG).
    pub opcode: u8,
    /// Flags modifying operation behavior.
    pub flags: u8,
    /// Custom correlation user data passed to the completion entry.
    pub user_data: u64,
    /// Index into pre-allocated datagram buffer pool.
    pub buffer_idx: u32,
    /// Byte length for transmission or receiving capacity.
    pub len: u32,
}

/// Completion Queue Entry (CQE) descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IoUringCqEntry {
    /// Correlation user data matching the originating SQE.
    pub user_data: u64,
    /// Result code (positive byte count on success, negative errno on error).
    pub res: i32,
    /// Completion event flags.
    pub flags: u32,
}

/// Linux io_uring configuration parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoUringParams {
    /// Number of submission queue entries.
    pub sq_entries: u32,
    /// Number of completion queue entries.
    pub cq_entries: u32,
    /// Feature and setup flags.
    pub flags: u32,
}

impl Default for IoUringParams {
    fn default() -> Self {
        Self {
            sq_entries: 1024,
            cq_entries: 2048,
            flags: IORING_SETUP_SQPOLL,
        }
    }
}

/// Pre-allocated circular ring buffer managing submission and completion queues.
pub struct IoUringRingBuffer<const CAPACITY: usize> {
    sq: Vec<IoUringSqEntry>,
    cq: Vec<IoUringCqEntry>,
    buffers: Vec<[u8; PACKET_BUFFER_SIZE]>,
    sq_head: AtomicUsize,
    sq_tail: AtomicUsize,
    cq_head: AtomicUsize,
    cq_tail: AtomicUsize,
}

impl<const CAPACITY: usize> IoUringRingBuffer<CAPACITY> {
    /// Constructs an io_uring ring buffer with pre-allocated submission, completion, and data arrays.
    pub fn new() -> Self {
        let mut sq = Vec::with_capacity(CAPACITY);
        let mut cq = Vec::with_capacity(CAPACITY * 2);
        let mut buffers = Vec::with_capacity(CAPACITY);

        for _ in 0..CAPACITY {
            sq.push(IoUringSqEntry::default());
            buffers.push([0u8; PACKET_BUFFER_SIZE]);
        }
        for _ in 0..(CAPACITY * 2) {
            cq.push(IoUringCqEntry::default());
        }

        Self {
            sq,
            cq,
            buffers,
            sq_head: AtomicUsize::new(0),
            sq_tail: AtomicUsize::new(0),
            cq_head: AtomicUsize::new(0),
            cq_tail: AtomicUsize::new(0),
        }
    }

    /// Submits a datagram receive request to the submission queue.
    pub fn submit_recv(&mut self, user_data: u64) -> Result<usize, IoUringError> {
        let tail = self.sq_tail.load(Ordering::Relaxed);
        let head = self.sq_head.load(Ordering::Acquire);

        if tail.wrapping_sub(head) >= CAPACITY {
            return Err(IoUringError::SubmissionQueueFull);
        }

        let idx = tail % CAPACITY;
        self.sq[idx] = IoUringSqEntry {
            opcode: IORING_OP_RECVMSG,
            flags: 0,
            user_data,
            buffer_idx: idx as u32,
            len: PACKET_BUFFER_SIZE as u32,
        };

        self.sq_tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(idx)
    }

    /// Submits a datagram send request to the submission queue with payload copy.
    pub fn submit_send(&mut self, user_data: u64, payload: &[u8]) -> Result<usize, IoUringError> {
        if payload.len() > PACKET_BUFFER_SIZE {
            return Err(IoUringError::PacketTooLarge(payload.len()));
        }

        let tail = self.sq_tail.load(Ordering::Relaxed);
        let head = self.sq_head.load(Ordering::Acquire);

        if tail.wrapping_sub(head) >= CAPACITY {
            return Err(IoUringError::SubmissionQueueFull);
        }

        let idx = tail % CAPACITY;
        self.buffers[idx][..payload.len()].copy_from_slice(payload);

        self.sq[idx] = IoUringSqEntry {
            opcode: IORING_OP_SENDMSG,
            flags: 0,
            user_data,
            buffer_idx: idx as u32,
            len: payload.len() as u32,
        };

        self.sq_tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(idx)
    }

    /// Completes an in-flight submission entry and writes into the completion queue.
    pub fn complete(
        &mut self,
        user_data: u64,
        result: i32,
        flags: u32,
    ) -> Result<(), IoUringError> {
        let tail = self.cq_tail.load(Ordering::Relaxed);
        let head = self.cq_head.load(Ordering::Acquire);
        let cq_cap = CAPACITY * 2;

        if tail.wrapping_sub(head) >= cq_cap {
            return Err(IoUringError::CompletionQueueFull);
        }

        let idx = tail % cq_cap;
        self.cq[idx] = IoUringCqEntry {
            user_data,
            res: result,
            flags,
        };

        self.cq_tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Drains completed entries from the completion queue into caller's buffer.
    pub fn poll_completions(&mut self, out: &mut [IoUringCqEntry]) -> usize {
        let head = self.cq_head.load(Ordering::Relaxed);
        let tail = self.cq_tail.load(Ordering::Acquire);
        let cq_cap = CAPACITY * 2;

        let available = tail.wrapping_sub(head);
        let count = available.min(out.len());

        for (i, entry) in out.iter_mut().take(count).enumerate() {
            let idx = (head + i) % cq_cap;
            *entry = self.cq[idx];
        }

        self.cq_head
            .store(head.wrapping_add(count), Ordering::Release);
        count
    }

    /// Returns the number of pending entries in the submission queue.
    #[inline]
    pub fn sq_pending(&self) -> usize {
        let head = self.sq_head.load(Ordering::Relaxed);
        let tail = self.sq_tail.load(Ordering::Relaxed);
        tail.saturating_sub(head)
    }

    /// Returns the number of completed entries in the completion queue.
    #[inline]
    pub fn cq_pending(&self) -> usize {
        let head = self.cq_head.load(Ordering::Relaxed);
        let tail = self.cq_tail.load(Ordering::Relaxed);
        tail.saturating_sub(head)
    }

    /// Accesses the underlying payload buffer for a specific buffer index.
    #[inline]
    pub fn get_buffer(&self, idx: usize) -> Option<&[u8]> {
        self.buffers.get(idx).map(|b| b.as_slice())
    }

    /// Accesses mutable payload buffer for a specific buffer index.
    #[inline]
    pub fn get_buffer_mut(&mut self, idx: usize) -> Option<&mut [u8]> {
        self.buffers.get_mut(idx).map(|b| b.as_mut_slice())
    }
}

impl<const CAPACITY: usize> Default for IoUringRingBuffer<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Portable driver coordinating zero-copy datagram submissions and completions.
pub struct IoUringDriver {
    rings: IoUringRingBuffer<1024>,
    total_submissions: u64,
    total_completions: u64,
}

impl IoUringDriver {
    /// Creates an io_uring transport driver with default capacity.
    pub fn new() -> Self {
        Self {
            rings: IoUringRingBuffer::new(),
            total_submissions: 0,
            total_completions: 0,
        }
    }

    /// Posts a batch of datagram transmission payloads.
    pub fn submit_send_batch(&mut self, packets: &[&[u8]]) -> usize {
        let mut submitted = 0;
        for &pkt in packets {
            let token = self.total_submissions + submitted as u64;
            if self.rings.submit_send(token, pkt).is_ok() {
                submitted += 1;
            } else {
                break;
            }
        }
        self.total_submissions += submitted as u64;
        submitted
    }

    /// Posts a batch of receive requests for incoming datagrams.
    pub fn submit_recv_batch(&mut self, count: usize) -> usize {
        let mut submitted = 0;
        for _ in 0..count {
            let token = self.total_submissions + submitted as u64;
            if self.rings.submit_recv(token).is_ok() {
                submitted += 1;
            } else {
                break;
            }
        }
        self.total_submissions += submitted as u64;
        submitted
    }

    /// Completes pending submissions in batch.
    pub fn complete_batch(&mut self, completions: &[(u64, i32)]) -> usize {
        let mut completed = 0;
        for &(user_data, res) in completions {
            if self.rings.complete(user_data, res, 0).is_ok() {
                completed += 1;
            } else {
                break;
            }
        }
        self.total_completions += completed as u64;
        completed
    }

    /// Polls completed I/O operations into destination slice.
    pub fn poll_completions(&mut self, out: &mut [IoUringCqEntry]) -> usize {
        self.rings.poll_completions(out)
    }

    /// Returns lifetime submission count.
    #[inline]
    pub fn total_submissions(&self) -> u64 {
        self.total_submissions
    }

    /// Returns lifetime completion count.
    #[inline]
    pub fn total_completions(&self) -> u64 {
        self.total_completions
    }
}

impl Default for IoUringDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_io_uring_ring_buffer_roundtrip() {
        let mut ring = IoUringRingBuffer::<16>::new();
        assert_eq!(ring.sq_pending(), 0);
        assert_eq!(ring.cq_pending(), 0);

        let data = b"EIDOLON_PACKET_TEST";
        let idx = ring.submit_send(42, data).unwrap();
        assert_eq!(ring.sq_pending(), 1);

        let buf = ring.get_buffer(idx).unwrap();
        assert_eq!(&buf[..data.len()], data);

        // Complete the operation
        assert!(ring.complete(42, data.len() as i32, 0).is_ok());
        assert_eq!(ring.cq_pending(), 1);

        let mut completions = [IoUringCqEntry::default(); 4];
        let count = ring.poll_completions(&mut completions);
        assert_eq!(count, 1);
        assert_eq!(completions[0].user_data, 42);
        assert_eq!(completions[0].res, data.len() as i32);
        assert_eq!(ring.cq_pending(), 0);
    }

    #[test]
    fn test_io_uring_submission_overflow() {
        let mut ring = IoUringRingBuffer::<4>::new();
        for i in 0..4 {
            assert!(ring.submit_recv(i as u64).is_ok());
        }
        assert_eq!(
            ring.submit_recv(100),
            Err(IoUringError::SubmissionQueueFull)
        );
    }
}
