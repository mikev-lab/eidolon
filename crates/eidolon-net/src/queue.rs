//! Fixed-capacity circular FIFO ring buffers and backpressure management.
//!
//! Enforces bounded memory limits per connected client with zero runtime heap allocations.

use crate::error::NetError;

/// Fixed-capacity circular FIFO ring buffer with deterministic bounds.
#[derive(Debug)]
pub struct PacketRingBuffer<T, const CAP: usize> {
    slots: [Option<T>; CAP],
    head: usize,
    tail: usize,
    len: usize,
}

impl<T, const CAP: usize> PacketRingBuffer<T, CAP> {
    /// Creates a new empty `PacketRingBuffer`.
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    /// Returns the number of items currently stored in the buffer.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the buffer contains no items.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns true if the buffer is at maximum capacity.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len == CAP
    }

    /// Returns the maximum capacity of the ring buffer.
    #[inline]
    pub fn capacity(&self) -> usize {
        CAP
    }

    /// Pushes an item into the buffer.
    ///
    /// Returns `Err(NetError::QueueFull)` if the buffer is already saturated.
    pub fn push(&mut self, item: T) -> Result<(), NetError> {
        if self.is_full() {
            return Err(NetError::QueueFull);
        }

        if let Some(slot) = self.slots.get_mut(self.tail) {
            *slot = Some(item);
            self.tail = (self.tail + 1) % CAP;
            self.len += 1;
            Ok(())
        } else {
            Err(NetError::QueueFull)
        }
    }

    /// Pushes an item into the buffer, discarding the oldest element if full.
    ///
    /// Returns `Some(old_item)` if an element was dropped to make room, or `None` if space was available.
    pub fn push_overwrite_oldest(&mut self, item: T) -> Option<T> {
        if CAP == 0 {
            return Some(item);
        }

        let dropped = if self.is_full() { self.pop() } else { None };

        if let Some(slot) = self.slots.get_mut(self.tail) {
            *slot = Some(item);
            self.tail = (self.tail + 1) % CAP;
            self.len += 1;
        }

        dropped
    }

    /// Removes and returns the oldest item from the buffer, or `None` if empty.
    pub fn pop(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }

        let item = self.slots.get_mut(self.head)?.take();
        self.head = (self.head + 1) % CAP;
        self.len = self.len.saturating_sub(1);
        item
    }

    /// Returns an immutable reference to the oldest item in the buffer without removing it.
    #[inline]
    pub fn peek(&self) -> Option<&T> {
        if self.is_empty() {
            None
        } else {
            self.slots.get(self.head)?.as_ref()
        }
    }

    /// Clears all items from the ring buffer.
    pub fn clear(&mut self) {
        while self.pop().is_some() {}
        self.head = 0;
        self.tail = 0;
        self.len = 0;
    }
}

impl<T, const CAP: usize> Default for PacketRingBuffer<T, CAP> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_fifo() {
        let mut queue = PacketRingBuffer::<u32, 4>::new();
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);

        assert!(queue.push(10).is_ok());
        assert!(queue.push(20).is_ok());
        assert!(queue.push(30).is_ok());
        assert!(queue.push(40).is_ok());
        assert!(queue.is_full());
        assert_eq!(queue.push(50), Err(NetError::QueueFull));

        assert_eq!(queue.peek(), Some(&10));
        assert_eq!(queue.pop(), Some(10));
        assert_eq!(queue.pop(), Some(20));
        assert_eq!(queue.len(), 2);

        assert!(queue.push(50).is_ok());
        assert!(queue.push(60).is_ok());

        assert_eq!(queue.pop(), Some(30));
        assert_eq!(queue.pop(), Some(40));
        assert_eq!(queue.pop(), Some(50));
        assert_eq!(queue.pop(), Some(60));
        assert_eq!(queue.pop(), None);
        assert!(queue.is_empty());
    }

    #[test]
    fn test_ring_buffer_overwrite_oldest() {
        let mut queue = PacketRingBuffer::<u32, 3>::new();
        assert_eq!(queue.push_overwrite_oldest(1), None);
        assert_eq!(queue.push_overwrite_oldest(2), None);
        assert_eq!(queue.push_overwrite_oldest(3), None);

        // Buffer full: pushing 4 should drop oldest (1)
        assert_eq!(queue.push_overwrite_oldest(4), Some(1));
        assert_eq!(queue.len(), 3);

        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), Some(4));
        assert_eq!(queue.pop(), None);
    }
}
