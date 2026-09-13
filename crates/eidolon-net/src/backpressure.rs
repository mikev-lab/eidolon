//! Bounded backpressure queues, saturation dropping policies, and pipeline monitoring.
//!
//! Enforces zero unbounded queues across network stages, dropping stale unsequenced
//! movement packets under ingress pressure and oldest unreliable positions under egress
//! backpressure, while strictly preserving ordered reliable packets.

use crate::error::NetError;

/// Multi-tier backpressure level representing pipeline saturation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum BackpressureLevel {
    /// Normal operation (<60% capacity utilization): all traffic admitted.
    #[default]
    Normal = 0,
    /// Elevated pressure (60% to 80% capacity): shed horizon AoI events.
    Elevated = 1,
    /// Saturated pressure (80% to 95% capacity): shed mid tier, drop stale movement.
    Saturated = 2,
    /// Critical saturation (>95% capacity): shed all non-immediate, apply hard backpressure.
    Critical = 3,
}

impl BackpressureLevel {
    /// Computes the backpressure level from a given utilization percentage (0 to 100).
    #[inline]
    pub const fn from_utilization(pct: u32) -> Self {
        if pct >= 95 {
            Self::Critical
        } else if pct >= 80 {
            Self::Saturated
        } else if pct >= 60 {
            Self::Elevated
        } else {
            Self::Normal
        }
    }
}

/// A prioritized packet wrapped with delivery semantics and sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrioritizedPacket<T> {
    /// Inner payload data.
    pub payload: T,
    /// Whether this packet requires reliable ordered delivery.
    pub is_reliable: bool,
    /// Monotonic sequence number for ordering and deduplication.
    pub sequence: u16,
}

impl<T> PrioritizedPacket<T> {
    /// Creates a new prioritized packet.
    #[inline]
    pub const fn new(payload: T, is_reliable: bool, sequence: u16) -> Self {
        Self {
            payload,
            is_reliable,
            sequence,
        }
    }
}

/// Backpressure metrics tracking drop counters and saturation events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BackpressureMetrics {
    /// Total reliable packets enqueued.
    pub reliable_enqueued: u64,
    /// Total unreliable packets enqueued.
    pub unreliable_enqueued: u64,
    /// Stale unsequenced ingress packets dropped under saturation.
    pub ingress_stale_dropped: u64,
    /// Oldest unreliable egress transforms dropped to make room for newer state.
    pub egress_unreliable_dropped: u64,
    /// Number of times queues reached saturated or critical levels.
    pub saturation_events: u64,
}

/// Bounded ingress queue differentiating movement packets from reliable commands.
///
/// Under backpressure: drops stale unsequenced movement packets, but preserves
/// reliable commands. If the queue is 100% full, reliable packets evict the oldest
/// unreliable packet to guarantee delivery.
#[derive(Debug)]
pub struct BoundedIngressQueue<T, const CAP: usize> {
    slots: [Option<PrioritizedPacket<T>>; CAP],
    head: usize,
    tail: usize,
    len: usize,
    reliable_count: usize,
    unreliable_count: usize,
    metrics: BackpressureMetrics,
}

impl<T: Copy, const CAP: usize> BoundedIngressQueue<T, CAP> {
    /// Creates a new empty bounded ingress queue.
    pub fn new() -> Self {
        Self {
            slots: [None; CAP],
            head: 0,
            tail: 0,
            len: 0,
            reliable_count: 0,
            unreliable_count: 0,
            metrics: BackpressureMetrics::default(),
        }
    }

    /// Returns the current number of packets in the queue.
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the queue is empty.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns true if the queue is full.
    #[inline]
    pub const fn is_full(&self) -> bool {
        self.len == CAP
    }

    /// Returns current capacity utilization percentage (0 to 100).
    #[inline]
    pub fn utilization_pct(&self) -> u32 {
        if CAP == 0 {
            return 100;
        }
        ((self.len as u64 * 100) / CAP as u64) as u32
    }

    /// Returns the current backpressure level of the queue.
    #[inline]
    pub fn backpressure_level(&self) -> BackpressureLevel {
        BackpressureLevel::from_utilization(self.utilization_pct())
    }

    /// Returns a copy of backpressure metrics.
    #[inline]
    pub const fn metrics(&self) -> BackpressureMetrics {
        self.metrics
    }

    /// Pushes an incoming packet into the ingress queue with saturation protection.
    ///
    /// If `is_reliable` is true:
    /// - If space is available, enqueued immediately.
    /// - If queue is full, evicts the oldest unreliable packet to guarantee delivery.
    /// - Returns `Err(NetError::QueueFull)` only if the queue is 100% saturated with reliable packets.
    ///
    /// If `is_reliable` is false:
    /// - Enqueued if space is available.
    /// - If queue is full or at Critical backpressure (>95%), dropped as stale movement.
    pub fn push(&mut self, payload: T, is_reliable: bool, sequence: u16) -> Result<(), NetError> {
        let packet = PrioritizedPacket::new(payload, is_reliable, sequence);

        if is_reliable {
            if self.is_full() {
                // Try to evict the oldest unreliable packet to make room for reliable command
                if self.unreliable_count > 0 {
                    self.evict_oldest_unreliable();
                } else {
                    self.metrics.saturation_events += 1;
                    return Err(NetError::QueueFull);
                }
            }

            self.enqueue_tail(packet);
            self.reliable_count += 1;
            self.metrics.reliable_enqueued += 1;
            Ok(())
        } else {
            // Unreliable movement packet
            if self.is_full() || self.utilization_pct() >= 95 {
                self.metrics.ingress_stale_dropped += 1;
                self.metrics.saturation_events += 1;
                return Err(NetError::QueueFull);
            }

            self.enqueue_tail(packet);
            self.unreliable_count += 1;
            self.metrics.unreliable_enqueued += 1;
            Ok(())
        }
    }

    /// Pops the next packet in FIFO order.
    pub fn pop(&mut self) -> Option<PrioritizedPacket<T>> {
        if self.len == 0 {
            return None;
        }

        let packet = self.slots.get_mut(self.head)?.take();
        self.head = (self.head + 1) % CAP;
        self.len = self.len.saturating_sub(1);

        if let Some(ref pkt) = packet {
            if pkt.is_reliable {
                self.reliable_count = self.reliable_count.saturating_sub(1);
            } else {
                self.unreliable_count = self.unreliable_count.saturating_sub(1);
            }
        }

        packet
    }

    /// Drains available packets into the destination slice, returning the number drained.
    pub fn drain_into(&mut self, dest: &mut [Option<PrioritizedPacket<T>>]) -> usize {
        let count = dest.len().min(self.len);
        for slot in dest.iter_mut().take(count) {
            *slot = self.pop();
        }
        count
    }

    #[inline]
    fn enqueue_tail(&mut self, packet: PrioritizedPacket<T>) {
        if let Some(slot) = self.slots.get_mut(self.tail) {
            *slot = Some(packet);
            self.tail = (self.tail + 1) % CAP;
            self.len += 1;
        }
    }

    /// Evicts the oldest unreliable packet from the queue, shifting elements to preserve order.
    fn evict_oldest_unreliable(&mut self) {
        if self.unreliable_count == 0 || self.len == 0 {
            return;
        }

        // Search from head for the first unreliable packet
        let mut target_idx = None;
        for i in 0..self.len {
            let idx = (self.head + i) % CAP;
            if let Some(ref pkt) = self.slots.get(idx).copied().flatten() {
                if !pkt.is_reliable {
                    target_idx = Some(idx);
                    break;
                }
            }
        }

        if let Some(unreliable_pos) = target_idx {
            // Shift elements between unreliable_pos and tail forward by one
            let mut curr = unreliable_pos;
            while curr != (self.tail.checked_sub(1).unwrap_or(CAP - 1)) {
                let next = (curr + 1) % CAP;
                let next_val = self.slots.get_mut(next).and_then(|s| s.take());
                if let Some(slot) = self.slots.get_mut(curr) {
                    *slot = next_val;
                }
                curr = next;
            }

            // Clear the old tail slot
            self.tail = self.tail.checked_sub(1).unwrap_or(CAP - 1);
            if let Some(slot) = self.slots.get_mut(self.tail) {
                *slot = None;
            }

            self.len = self.len.saturating_sub(1);
            self.unreliable_count = self.unreliable_count.saturating_sub(1);
            self.metrics.ingress_stale_dropped += 1;
        }
    }
}

impl<T: Copy, const CAP: usize> Default for BoundedIngressQueue<T, CAP> {
    fn default() -> Self {
        Self::new()
    }
}

/// Bounded egress queue with oldest-unreliable dropping under backpressure.
///
/// When saturated, drops oldest unacked unreliable transforms in favor of newer
/// coordinates, while guaranteeing that ordered reliable packets are never dropped.
#[derive(Debug)]
pub struct BoundedEgressQueue<T, const CAP: usize> {
    slots: [Option<PrioritizedPacket<T>>; CAP],
    head: usize,
    tail: usize,
    len: usize,
    reliable_count: usize,
    unreliable_count: usize,
    metrics: BackpressureMetrics,
}

impl<T: Copy, const CAP: usize> BoundedEgressQueue<T, CAP> {
    /// Creates a new empty bounded egress queue.
    pub fn new() -> Self {
        Self {
            slots: [None; CAP],
            head: 0,
            tail: 0,
            len: 0,
            reliable_count: 0,
            unreliable_count: 0,
            metrics: BackpressureMetrics::default(),
        }
    }

    /// Returns the current number of packets in the queue.
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the queue is empty.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns true if the queue is full.
    #[inline]
    pub const fn is_full(&self) -> bool {
        self.len == CAP
    }

    /// Returns capacity utilization percentage (0 to 100).
    #[inline]
    pub fn utilization_pct(&self) -> u32 {
        if CAP == 0 {
            return 100;
        }
        ((self.len as u64 * 100) / CAP as u64) as u32
    }

    /// Returns current backpressure level.
    #[inline]
    pub fn backpressure_level(&self) -> BackpressureLevel {
        BackpressureLevel::from_utilization(self.utilization_pct())
    }

    /// Returns a copy of backpressure metrics.
    #[inline]
    pub const fn metrics(&self) -> BackpressureMetrics {
        self.metrics
    }

    /// Pushes an egress packet with priority-based saturation handling.
    ///
    /// - If unreliable: when full, drops the oldest unreliable transform to enqueue
    ///   the freshest position. If the queue is 100% reliable, returns `Err(NetError::QueueFull)`.
    /// - If reliable: when full, evicts the oldest unreliable transform to make room.
    ///   Returns `Err(NetError::QueueFull)` only when 100% full of reliable packets.
    pub fn push(&mut self, payload: T, is_reliable: bool, sequence: u16) -> Result<(), NetError> {
        let packet = PrioritizedPacket::new(payload, is_reliable, sequence);

        if self.is_full() {
            if self.unreliable_count > 0 {
                self.evict_oldest_unreliable();
                self.metrics.egress_unreliable_dropped += 1;
            } else {
                self.metrics.saturation_events += 1;
                return Err(NetError::QueueFull);
            }
        }

        if let Some(slot) = self.slots.get_mut(self.tail) {
            *slot = Some(packet);
            self.tail = (self.tail + 1) % CAP;
            self.len += 1;

            if is_reliable {
                self.reliable_count += 1;
                self.metrics.reliable_enqueued += 1;
            } else {
                self.unreliable_count += 1;
                self.metrics.unreliable_enqueued += 1;
            }
            Ok(())
        } else {
            Err(NetError::QueueFull)
        }
    }

    /// Pops the next packet in FIFO order.
    pub fn pop(&mut self) -> Option<PrioritizedPacket<T>> {
        if self.len == 0 {
            return None;
        }

        let packet = self.slots.get_mut(self.head)?.take();
        self.head = (self.head + 1) % CAP;
        self.len = self.len.saturating_sub(1);

        if let Some(ref pkt) = packet {
            if pkt.is_reliable {
                self.reliable_count = self.reliable_count.saturating_sub(1);
            } else {
                self.unreliable_count = self.unreliable_count.saturating_sub(1);
            }
        }

        packet
    }

    /// Drains available packets into the destination slice, returning count drained.
    pub fn drain_into(&mut self, dest: &mut [Option<PrioritizedPacket<T>>]) -> usize {
        let count = dest.len().min(self.len);
        for slot in dest.iter_mut().take(count) {
            *slot = self.pop();
        }
        count
    }

    fn evict_oldest_unreliable(&mut self) {
        if self.unreliable_count == 0 || self.len == 0 {
            return;
        }

        let mut target_idx = None;
        for i in 0..self.len {
            let idx = (self.head + i) % CAP;
            if let Some(ref pkt) = self.slots.get(idx).copied().flatten() {
                if !pkt.is_reliable {
                    target_idx = Some(idx);
                    break;
                }
            }
        }

        if let Some(unreliable_pos) = target_idx {
            let mut curr = unreliable_pos;
            while curr != (self.tail.checked_sub(1).unwrap_or(CAP - 1)) {
                let next = (curr + 1) % CAP;
                let next_val = self.slots.get_mut(next).and_then(|s| s.take());
                if let Some(slot) = self.slots.get_mut(curr) {
                    *slot = next_val;
                }
                curr = next;
            }

            self.tail = self.tail.checked_sub(1).unwrap_or(CAP - 1);
            if let Some(slot) = self.slots.get_mut(self.tail) {
                *slot = None;
            }

            self.len = self.len.saturating_sub(1);
            self.unreliable_count = self.unreliable_count.saturating_sub(1);
        }
    }
}

impl<T: Copy, const CAP: usize> Default for BoundedEgressQueue<T, CAP> {
    fn default() -> Self {
        Self::new()
    }
}

/// System-wide backpressure coordinator evaluating utilization across all pipeline stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BackpressureCoordinator {
    ingress_pct: u32,
    sim_pct: u32,
    egress_pct: u32,
    effective_level: BackpressureLevel,
}

impl BackpressureCoordinator {
    /// Creates a new coordinator initialized to Normal level.
    pub const fn new() -> Self {
        Self {
            ingress_pct: 0,
            sim_pct: 0,
            egress_pct: 0,
            effective_level: BackpressureLevel::Normal,
        }
    }

    /// Updates queue utilization percentages and computes system-wide backpressure level.
    pub fn update_pipeline(
        &mut self,
        ingress_pct: u32,
        sim_pct: u32,
        egress_pct: u32,
    ) -> BackpressureLevel {
        self.ingress_pct = ingress_pct.min(100);
        self.sim_pct = sim_pct.min(100);
        self.egress_pct = egress_pct.min(100);

        // Maximum utilization dictates effective backpressure level
        let max_pct = self.ingress_pct.max(self.sim_pct).max(self.egress_pct);
        self.effective_level = BackpressureLevel::from_utilization(max_pct);
        self.effective_level
    }

    /// Returns the active effective backpressure level.
    #[inline]
    pub const fn effective_level(self) -> BackpressureLevel {
        self.effective_level
    }

    /// Returns true if distant Horizon AoI events should be shed.
    #[inline]
    pub const fn should_shed_horizon(self) -> bool {
        (self.effective_level as u8) >= (BackpressureLevel::Elevated as u8)
    }

    /// Returns true if Mid-range AoI events should be throttled or shed.
    #[inline]
    pub const fn should_shed_mid_range(self) -> bool {
        (self.effective_level as u8) >= (BackpressureLevel::Saturated as u8)
    }

    /// Returns true if movement updates should be aggressively decimated.
    #[inline]
    pub const fn should_decimate_movement(self) -> bool {
        (self.effective_level as u8) >= (BackpressureLevel::Critical as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backpressure_level_thresholds() {
        assert_eq!(
            BackpressureLevel::from_utilization(0),
            BackpressureLevel::Normal
        );
        assert_eq!(
            BackpressureLevel::from_utilization(59),
            BackpressureLevel::Normal
        );
        assert_eq!(
            BackpressureLevel::from_utilization(60),
            BackpressureLevel::Elevated
        );
        assert_eq!(
            BackpressureLevel::from_utilization(79),
            BackpressureLevel::Elevated
        );
        assert_eq!(
            BackpressureLevel::from_utilization(80),
            BackpressureLevel::Saturated
        );
        assert_eq!(
            BackpressureLevel::from_utilization(94),
            BackpressureLevel::Saturated
        );
        assert_eq!(
            BackpressureLevel::from_utilization(95),
            BackpressureLevel::Critical
        );
        assert_eq!(
            BackpressureLevel::from_utilization(100),
            BackpressureLevel::Critical
        );
    }

    #[test]
    fn test_ingress_queue_reliable_evicts_unreliable_when_full() {
        let mut queue: BoundedIngressQueue<u32, 4> = BoundedIngressQueue::new();

        // Enqueue 4 unreliable packets (fills queue)
        assert!(queue.push(10, false, 1).is_ok());
        assert!(queue.push(20, false, 2).is_ok());
        assert!(queue.push(30, false, 3).is_ok());
        assert!(queue.push(40, false, 4).is_ok());
        assert!(queue.is_full());

        // Attempting to push 5th unreliable packet fails
        assert_eq!(queue.push(50, false, 5), Err(NetError::QueueFull));

        // Pushing a reliable packet succeeds by evicting the oldest unreliable packet (10)
        assert!(queue.push(999, true, 100).is_ok());
        assert_eq!(queue.len(), 4);
        assert_eq!(queue.metrics().ingress_stale_dropped, 2); // 1 from failed push + 1 from eviction

        // First popped item should be 20 (since 10 was evicted)
        let p1 = queue.pop().unwrap();
        assert_eq!(p1.payload, 20);
        assert!(!p1.is_reliable);

        let p2 = queue.pop().unwrap();
        assert_eq!(p2.payload, 30);

        let p3 = queue.pop().unwrap();
        assert_eq!(p3.payload, 40);

        let p4 = queue.pop().unwrap();
        assert_eq!(p4.payload, 999);
        assert!(p4.is_reliable);

        assert!(queue.is_empty());
    }

    #[test]
    fn test_egress_queue_drops_oldest_unreliable_for_fresh_state() {
        let mut queue: BoundedEgressQueue<u32, 3> = BoundedEgressQueue::new();

        // Enqueue 1 reliable, 2 unreliable
        assert!(queue.push(100, true, 1).is_ok());
        assert!(queue.push(1, false, 2).is_ok());
        assert!(queue.push(2, false, 3).is_ok());
        assert!(queue.is_full());

        // Pushing fresh position 3 drops oldest unreliable (1), preserves reliable (100)
        assert!(queue.push(3, false, 4).is_ok());
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.metrics().egress_unreliable_dropped, 1);

        // Verify queue contains 100, 2, 3 in order
        let p1 = queue.pop().unwrap();
        assert_eq!(p1.payload, 100);
        assert!(p1.is_reliable);

        let p2 = queue.pop().unwrap();
        assert_eq!(p2.payload, 2);
        assert!(!p2.is_reliable);

        let p3 = queue.pop().unwrap();
        assert_eq!(p3.payload, 3);
        assert!(!p3.is_reliable);
    }

    #[test]
    fn test_backpressure_coordinator_shedding_flags() {
        let mut coord = BackpressureCoordinator::new();
        assert_eq!(coord.update_pipeline(20, 30, 40), BackpressureLevel::Normal);
        assert!(!coord.should_shed_horizon());
        assert!(!coord.should_shed_mid_range());

        assert_eq!(
            coord.update_pipeline(65, 30, 40),
            BackpressureLevel::Elevated
        );
        assert!(coord.should_shed_horizon());
        assert!(!coord.should_shed_mid_range());

        assert_eq!(
            coord.update_pipeline(65, 85, 40),
            BackpressureLevel::Saturated
        );
        assert!(coord.should_shed_horizon());
        assert!(coord.should_shed_mid_range());
        assert!(!coord.should_decimate_movement());

        assert_eq!(
            coord.update_pipeline(65, 85, 96),
            BackpressureLevel::Critical
        );
        assert!(coord.should_decimate_movement());
    }
}
