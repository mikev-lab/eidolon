//! Correlated distributed tracing context, span telemetry, and zero-allocation ring buffers.
//!
//! Enables end-to-end trace context propagation across the request lifecycle:
//! Client Input -> Gateway -> Zone -> Simulation -> AoI -> Egress.
//! Provides microsecond span attribution for diagnosing player rubber-banding, input lag, and seam stalls.

/// Seven-part correlation tuple linking an operation across distributed MMO server tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TraceContext {
    /// Client connection session identifier.
    pub session_id: u32,
    /// Authoritative simulation tick index.
    pub tick_id: u64,
    /// Entity unique identifier.
    pub entity_id: u32,
    /// World zone identifier.
    pub zone_id: u16,
    /// Monotonic distributed authority epoch.
    pub authority_epoch: u32,
    /// Root trace identifier spanning the complete request lifecycle.
    pub trace_id: u64,
    /// Current span identifier within the trace.
    pub span_id: u32,
}

impl TraceContext {
    /// Creates a new root trace context.
    #[inline]
    pub const fn new(
        session_id: u32,
        tick_id: u64,
        entity_id: u32,
        zone_id: u16,
        authority_epoch: u32,
        trace_id: u64,
        span_id: u32,
    ) -> Self {
        Self {
            session_id,
            tick_id,
            entity_id,
            zone_id,
            authority_epoch,
            trace_id,
            span_id,
        }
    }

    /// Derives a child span context preserving root correlation IDs while assigning a new span ID.
    #[inline]
    pub const fn child_span(&self, next_span_id: u32) -> Self {
        Self {
            session_id: self.session_id,
            tick_id: self.tick_id,
            entity_id: self.entity_id,
            zone_id: self.zone_id,
            authority_epoch: self.authority_epoch,
            trace_id: self.trace_id,
            span_id: next_span_id,
        }
    }
}

/// An instrumented execution span recording timing and correlation context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceSpan {
    /// Static identifier for the executed phase or operation.
    pub name: &'static str,
    /// Microsecond timestamp relative to simulation startup.
    pub start_micros: u64,
    /// Execution duration in microseconds.
    pub duration_micros: u32,
    /// Correlation context.
    pub context: TraceContext,
}

impl TraceSpan {
    /// Creates a new completed trace span.
    #[inline]
    pub const fn new(
        name: &'static str,
        start_micros: u64,
        duration_micros: u32,
        context: TraceContext,
    ) -> Self {
        Self {
            name,
            start_micros,
            duration_micros,
            context,
        }
    }
}

/// Fixed-capacity circular trace ring buffer for zero-allocation post-mortem diagnostics.
#[derive(Debug)]
pub struct TraceRingBuffer<const CAP: usize> {
    slots: [Option<TraceSpan>; CAP],
    head: usize,
    tail: usize,
    len: usize,
    total_recorded: u64,
}

impl<const CAP: usize> TraceRingBuffer<CAP> {
    /// Creates a new empty trace ring buffer.
    pub fn new() -> Self {
        Self {
            slots: [None; CAP],
            head: 0,
            tail: 0,
            len: 0,
            total_recorded: 0,
        }
    }

    /// Returns the number of spans currently buffered.
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the buffer contains no spans.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns true if the buffer has reached maximum capacity.
    #[inline]
    pub const fn is_full(&self) -> bool {
        self.len == CAP
    }

    /// Returns the total number of spans recorded across the buffer lifecycle.
    #[inline]
    pub const fn total_recorded(&self) -> u64 {
        self.total_recorded
    }

    /// Pushes a span into the ring buffer, overwriting the oldest span if full.
    pub fn push(&mut self, span: TraceSpan) {
        if CAP == 0 {
            return;
        }

        if self.is_full() {
            // Overwrite oldest: advance head
            self.head = (self.head + 1) % CAP;
        } else {
            self.len += 1;
        }

        if let Some(slot) = self.slots.get_mut(self.tail) {
            *slot = Some(span);
            self.tail = (self.tail + 1) % CAP;
            self.total_recorded += 1;
        }
    }

    /// Pops the oldest span in FIFO order.
    pub fn pop(&mut self) -> Option<TraceSpan> {
        if self.len == 0 {
            return None;
        }

        let span = self.slots.get_mut(self.head)?.take();
        self.head = (self.head + 1) % CAP;
        self.len = self.len.saturating_sub(1);
        span
    }

    /// Drains up to `dest.len()` spans in chronological order.
    pub fn drain_into(&mut self, dest: &mut [Option<TraceSpan>]) -> usize {
        let count = dest.len().min(self.len);
        for slot in dest.iter_mut().take(count) {
            *slot = self.pop();
        }
        count
    }

    /// Collects all spans matching the specified trace ID into `dest`, returning count matched.
    pub fn find_by_trace_id(&self, trace_id: u64, dest: &mut [TraceSpan]) -> usize {
        let mut matched = 0;
        for i in 0..self.len {
            if matched >= dest.len() {
                break;
            }
            let idx = (self.head + i) % CAP;
            if let Some(ref span) = self.slots.get(idx).copied().flatten() {
                if span.context.trace_id == trace_id {
                    if let Some(slot) = dest.get_mut(matched) {
                        *slot = *span;
                        matched += 1;
                    }
                }
            }
        }
        matched
    }
}

impl<const CAP: usize> Default for TraceRingBuffer<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_context_child_span_derivation() {
        let root = TraceContext::new(101, 500, 202, 1, 3, 0xABCD_EF01, 1);
        assert_eq!(root.session_id, 101);
        assert_eq!(root.tick_id, 500);
        assert_eq!(root.entity_id, 202);
        assert_eq!(root.zone_id, 1);
        assert_eq!(root.authority_epoch, 3);
        assert_eq!(root.trace_id, 0xABCD_EF01);
        assert_eq!(root.span_id, 1);

        let child = root.child_span(2);
        assert_eq!(child.session_id, 101);
        assert_eq!(child.trace_id, 0xABCD_EF01);
        assert_eq!(child.span_id, 2);
    }

    #[test]
    fn test_trace_ring_buffer_fifo_and_overwrite() {
        let mut buffer: TraceRingBuffer<3> = TraceRingBuffer::new();

        let ctx1 = TraceContext::new(1, 10, 100, 1, 1, 1001, 1);
        let ctx2 = TraceContext::new(2, 10, 200, 1, 1, 1002, 1);
        let ctx3 = TraceContext::new(3, 10, 300, 1, 1, 1003, 1);
        let ctx4 = TraceContext::new(4, 10, 400, 1, 1, 1004, 1);

        buffer.push(TraceSpan::new("ingress", 100, 15, ctx1));
        buffer.push(TraceSpan::new("sim", 115, 20, ctx2));
        buffer.push(TraceSpan::new("spatial", 135, 10, ctx3));
        assert!(buffer.is_full());
        assert_eq!(buffer.len(), 3);

        // Push 4th span: overwrites ctx1 ("ingress")
        buffer.push(TraceSpan::new("egress", 145, 12, ctx4));
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.total_recorded(), 4);

        // First popped span should be ctx2 ("sim")
        let s1 = buffer.pop().unwrap();
        assert_eq!(s1.name, "sim");
        assert_eq!(s1.context.trace_id, 1002);

        let s2 = buffer.pop().unwrap();
        assert_eq!(s2.name, "spatial");

        let s3 = buffer.pop().unwrap();
        assert_eq!(s3.name, "egress");

        assert!(buffer.is_empty());
    }

    #[test]
    fn test_trace_ring_buffer_find_by_trace_id() {
        let mut buffer: TraceRingBuffer<10> = TraceRingBuffer::new();
        let target_trace = 0xDEAD_BEEF;

        let ctx_target1 = TraceContext::new(1, 10, 100, 1, 1, target_trace, 1);
        let ctx_other = TraceContext::new(2, 10, 200, 1, 1, 0x1234, 1);
        let ctx_target2 = TraceContext::new(1, 10, 100, 1, 1, target_trace, 2);

        buffer.push(TraceSpan::new("ingress_receive", 100, 10, ctx_target1));
        buffer.push(TraceSpan::new("other_traffic", 110, 5, ctx_other));
        buffer.push(TraceSpan::new("egress_dispatch", 115, 12, ctx_target2));

        let mut results = [TraceSpan::new("", 0, 0, TraceContext::default()); 4];
        let matched = buffer.find_by_trace_id(target_trace, &mut results);

        assert_eq!(matched, 2);
        assert_eq!(results[0].name, "ingress_receive");
        assert_eq!(results[1].name, "egress_dispatch");
    }
}
