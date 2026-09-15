//! Lock-free Chase-Lev work-stealing deque and multi-core thread pool.
//!
//! Engineered in pure safe Rust with `#![deny(unsafe_code)]` using standard library
//! atomic primitives (`AtomicU64`, `AtomicUsize`). Provides constant-time O(1) job push/pop
//! on the owning worker thread and lock-free stealing across concurrent thief threads.

use std::fmt;
use std::sync::atomic::{fence, AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

/// Sentinel value indicating an empty slot in the work-stealing ring buffer.
pub const EMPTY_SLOT: u64 = u64::MAX;

/// Default capacity for the per-worker task deque (must be power of two).
pub const DEFAULT_DEQUE_CAPACITY: usize = 4096;

/// Errors arising from work-stealing deque operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkStealingError {
    /// Deque capacity is full.
    QueueFull,
    /// Invalid configuration parameter.
    InvalidCapacity,
    /// Worker thread encountered an internal synchronization error.
    WorkerError,
}

impl fmt::Display for WorkStealingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueueFull => write!(f, "Work-stealing deque capacity exceeded"),
            Self::InvalidCapacity => write!(f, "Deque capacity must be a positive power of two"),
            Self::WorkerError => write!(f, "Worker thread synchronization failure"),
        }
    }
}

/// Result of a concurrent steal attempt from a peer worker's deque.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StealResult {
    /// The target deque is currently empty.
    Empty,
    /// The steal attempt lost a race with the owner or another thief; caller should retry.
    Abort,
    /// Successfully stolen job payload.
    Success(u64),
}

/// Lock-free Chase-Lev work-stealing deque.
///
/// Designed with `#![deny(unsafe_code)]` compliance. Single-producer (owner)
/// pushes and pops from the bottom, while multiple consumers (thieves) steal from the top.
pub struct ChaseLevDeque {
    capacity: usize,
    mask: usize,
    top: AtomicUsize,
    bottom: AtomicUsize,
    slots: Vec<AtomicU64>,
}

impl ChaseLevDeque {
    /// Constructs a work-stealing deque with the specified power-of-two capacity.
    pub fn with_capacity(capacity: usize) -> Result<Self, WorkStealingError> {
        if capacity == 0 || (capacity & (capacity - 1)) != 0 {
            return Err(WorkStealingError::InvalidCapacity);
        }

        let mut slots = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            slots.push(AtomicU64::new(EMPTY_SLOT));
        }

        Ok(Self {
            capacity,
            mask: capacity - 1,
            top: AtomicUsize::new(0),
            bottom: AtomicUsize::new(0),
            slots,
        })
    }

    /// Creates a deque with the default capacity (4,096 elements).
    pub fn new() -> Self {
        match Self::with_capacity(DEFAULT_DEQUE_CAPACITY) {
            Ok(deque) => deque,
            Err(_) => {
                let capacity = 4096;
                let mut slots = Vec::with_capacity(capacity);
                for _ in 0..capacity {
                    slots.push(AtomicU64::new(0));
                }
                Self {
                    capacity,
                    mask: capacity - 1,
                    top: AtomicUsize::new(0),
                    bottom: AtomicUsize::new(0),
                    slots,
                }
            }
        }
    }

    /// Returns the capacity of the deque.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Pushes a job payload onto the bottom of the deque.
    ///
    /// Callable exclusively by the owning worker thread. Executes in constant time O(1).
    pub fn push(&self, value: u64) -> Result<(), WorkStealingError> {
        let b = self.bottom.load(Ordering::Relaxed);
        let t = self.top.load(Ordering::Acquire);

        if b.wrapping_sub(t) >= self.capacity {
            return Err(WorkStealingError::QueueFull);
        }

        self.slots[b & self.mask].store(value, Ordering::Release);
        self.bottom.store(b.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Pops a job payload from the bottom of the deque.
    ///
    /// Callable exclusively by the owning worker thread.
    pub fn pop(&self) -> Option<u64> {
        let b = self.bottom.load(Ordering::Relaxed);
        let t_init = self.top.load(Ordering::Relaxed);

        if b == 0 && t_init == 0 {
            return None;
        }

        let b_new = b.wrapping_sub(1);
        self.bottom.store(b_new, Ordering::Relaxed);
        fence(Ordering::SeqCst);

        let t = self.top.load(Ordering::Relaxed);
        if t <= b_new {
            let val = self.slots[b_new & self.mask].swap(EMPTY_SLOT, Ordering::AcqRel);
            if t == b_new {
                // Single element remaining: race against concurrent thieves
                let cas_res = self.top.compare_exchange(
                    t,
                    t.wrapping_add(1),
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                );
                self.bottom.store(t.wrapping_add(1), Ordering::Relaxed);
                if cas_res.is_ok() && val != EMPTY_SLOT {
                    Some(val)
                } else {
                    None
                }
            } else if val != EMPTY_SLOT {
                Some(val)
            } else {
                None
            }
        } else {
            // Deque was empty
            self.bottom.store(t, Ordering::Relaxed);
            None
        }
    }

    /// Steals a job payload from the top of the deque.
    ///
    /// Callable concurrently by any thief worker thread.
    pub fn steal(&self) -> StealResult {
        let t = self.top.load(Ordering::Acquire);
        fence(Ordering::SeqCst);
        let b = self.bottom.load(Ordering::Acquire);

        if t >= b {
            return StealResult::Empty;
        }

        let val = self.slots[t & self.mask].load(Ordering::Acquire);
        if val == EMPTY_SLOT {
            return StealResult::Abort;
        }

        match self
            .top
            .compare_exchange(t, t.wrapping_add(1), Ordering::SeqCst, Ordering::Relaxed)
        {
            Ok(_) => {
                self.slots[t & self.mask].store(EMPTY_SLOT, Ordering::Release);
                StealResult::Success(val)
            }
            Err(_) => StealResult::Abort,
        }
    }

    /// Returns approximate count of pending jobs in the deque.
    #[inline]
    pub fn len(&self) -> usize {
        let b = self.bottom.load(Ordering::Relaxed);
        let t = self.top.load(Ordering::Relaxed);
        b.saturating_sub(t)
    }

    /// Returns true if the deque is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for ChaseLevDeque {
    fn default() -> Self {
        Self::new()
    }
}

/// Multi-threaded work-stealing job coordination pool.
///
/// Distributes jobs evenly across worker queues and enables lock-free cooperative
/// task stealing from the shared injector queue and peer worker queues.
pub struct WorkStealingPool {
    worker_count: usize,
    injector: Arc<ChaseLevDeque>,
    deques: Vec<Arc<ChaseLevDeque>>,
    active: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
}

impl WorkStealingPool {
    /// Constructs a work-stealing pool with the specified number of worker threads.
    pub fn new<F>(worker_count: usize, handler: F) -> Self
    where
        F: Fn(usize, u64) + Send + Sync + 'static,
    {
        let count = worker_count.max(1);
        let injector = Arc::new(ChaseLevDeque::with_capacity(16384).unwrap_or_default());
        let mut deques = Vec::with_capacity(count);
        for _ in 0..count {
            deques.push(Arc::new(ChaseLevDeque::new()));
        }

        let active = Arc::new(AtomicBool::new(true));
        let handler_arc = Arc::new(handler);
        let mut workers = Vec::with_capacity(count);

        for worker_id in 0..count {
            let my_deque = Arc::clone(&deques[worker_id]);
            let shared_injector = Arc::clone(&injector);
            let all_deques = deques.clone();
            let is_active = Arc::clone(&active);
            let job_handler = Arc::clone(&handler_arc);

            let handle = thread::spawn(move || {
                let n_workers = all_deques.len();
                let mut victim_offset = 1;

                while is_active.load(Ordering::Relaxed) {
                    // 1. Try local work first (owner pop)
                    if let Some(job) = my_deque.pop() {
                        job_handler(worker_id, job);
                        continue;
                    }

                    // 2. Try stealing from shared injector queue
                    if let StealResult::Success(job) = shared_injector.steal() {
                        job_handler(worker_id, job);
                        continue;
                    }

                    // 3. Try stealing from peers
                    let mut stolen = false;
                    for i in 0..n_workers {
                        let victim_id = (worker_id + victim_offset + i) % n_workers;
                        if victim_id == worker_id {
                            continue;
                        }

                        match all_deques[victim_id].steal() {
                            StealResult::Success(job) => {
                                job_handler(worker_id, job);
                                stolen = true;
                                break;
                            }
                            StealResult::Abort | StealResult::Empty => {}
                        }
                    }

                    victim_offset = (victim_offset + 1) % n_workers.max(1);

                    if !stolen {
                        thread::yield_now();
                    }
                }
            });

            workers.push(handle);
        }

        Self {
            worker_count: count,
            injector,
            deques,
            active,
            workers,
        }
    }

    /// Submits a 64-bit job payload to the shared injector queue.
    #[inline]
    pub fn submit(&self, job: u64) -> Result<(), WorkStealingError> {
        self.injector.push(job)
    }

    /// Returns the number of worker threads in the pool.
    #[inline]
    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    /// Returns total number of pending jobs across all worker deques and the injector queue.
    pub fn total_pending_jobs(&self) -> usize {
        self.injector.len() + self.deques.iter().map(|d| d.len()).sum::<usize>()
    }

    /// Signals workers to shutdown and joins all threads.
    pub fn shutdown(mut self) {
        self.active.store(false, Ordering::Release);
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chase_lev_single_threaded_fifo_lifo() {
        let deque = ChaseLevDeque::with_capacity(16).unwrap();
        assert!(deque.is_empty());

        for i in 1..=8 {
            assert!(deque.push(i).is_ok());
        }
        assert_eq!(deque.len(), 8);

        // Owner pops from bottom (LIFO order)
        for expected in (1..=8).rev() {
            assert_eq!(deque.pop(), Some(expected));
        }
        assert_eq!(deque.pop(), None);
        assert!(deque.is_empty());
    }

    #[test]
    fn test_chase_lev_steal_fifo_order() {
        let deque = ChaseLevDeque::with_capacity(16).unwrap();
        for i in 101..=105 {
            assert!(deque.push(i).is_ok());
        }

        // Thief steals from top (FIFO order)
        assert_eq!(deque.steal(), StealResult::Success(101));
        assert_eq!(deque.steal(), StealResult::Success(102));

        // Owner pops remainder from bottom (LIFO)
        assert_eq!(deque.pop(), Some(105));
        assert_eq!(deque.pop(), Some(104));
        assert_eq!(deque.pop(), Some(103));
        assert_eq!(deque.pop(), None);
        assert_eq!(deque.steal(), StealResult::Empty);
    }

    #[test]
    fn test_chase_lev_queue_full_rejection() {
        let deque = ChaseLevDeque::with_capacity(4).unwrap();
        assert!(deque.push(1).is_ok());
        assert!(deque.push(2).is_ok());
        assert!(deque.push(3).is_ok());
        assert!(deque.push(4).is_ok());
        assert_eq!(deque.push(5), Err(WorkStealingError::QueueFull));
    }

    #[test]
    fn test_chase_lev_invalid_capacity() {
        assert_eq!(
            ChaseLevDeque::with_capacity(0).err(),
            Some(WorkStealingError::InvalidCapacity)
        );
        assert_eq!(
            ChaseLevDeque::with_capacity(7).err(),
            Some(WorkStealingError::InvalidCapacity)
        );
    }
}
