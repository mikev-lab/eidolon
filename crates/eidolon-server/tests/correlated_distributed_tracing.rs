//! Milestone 11.2 Automated Integration Test Suite: Correlated Distributed Tracing & Telemetry.
//!
//! Validates seven-part correlation tuple propagation across request lifecycles:
//! Client Input -> Gateway -> Zone -> Simulation -> AoI -> Egress.
//! Asserts zero-allocation ring buffer circular overwrite and microsecond span attribution.

use eidolon_core::trace::{TraceContext, TraceRingBuffer, TraceSpan};

#[test]
fn test_milestone_11_2_trace_context_lifecycle_propagation() {
    let mut ring_buffer: TraceRingBuffer<64> = TraceRingBuffer::new();

    // 1. Ingress at Gateway
    let trace_id = 0xA1B2_C3D4_E5F6_0001;
    let root_ctx = TraceContext::new(
        1001, // session_id
        50,   // tick_id
        2002, // entity_id
        1,    // zone_id
        5,    // authority_epoch
        trace_id, 1, // span_id
    );
    ring_buffer.push(TraceSpan::new("gateway_ingress", 1_000, 150, root_ctx));

    // 2. Zone Routing
    let span2_ctx = root_ctx.child_span(2);
    ring_buffer.push(TraceSpan::new("zone_route", 1_150, 80, span2_ctx));

    // 3. Authoritative Simulation
    let span3_ctx = root_ctx.child_span(3);
    ring_buffer.push(TraceSpan::new("simulation_step", 1_230, 450, span3_ctx));

    // 4. AoI Spatial Query
    let span4_ctx = root_ctx.child_span(4);
    ring_buffer.push(TraceSpan::new("spatial_aoi_query", 1_680, 220, span4_ctx));

    // 5. Egress Dispatch
    let span5_ctx = root_ctx.child_span(5);
    ring_buffer.push(TraceSpan::new("egress_dispatch", 1_900, 110, span5_ctx));

    assert_eq!(ring_buffer.len(), 5);

    // Query spans for root trace_id
    let mut trace_spans = [TraceSpan::new("", 0, 0, TraceContext::default()); 8];
    let matched = ring_buffer.find_by_trace_id(trace_id, &mut trace_spans);

    assert_eq!(matched, 5);
    assert_eq!(trace_spans[0].name, "gateway_ingress");
    assert_eq!(trace_spans[1].name, "zone_route");
    assert_eq!(trace_spans[2].name, "simulation_step");
    assert_eq!(trace_spans[3].name, "spatial_aoi_query");
    assert_eq!(trace_spans[4].name, "egress_dispatch");

    // All spans must maintain identical correlation context
    for span in &trace_spans[..5] {
        assert_eq!(span.context.trace_id, trace_id);
        assert_eq!(span.context.session_id, 1001);
        assert_eq!(span.context.entity_id, 2002);
        assert_eq!(span.context.zone_id, 1);
        assert_eq!(span.context.authority_epoch, 5);
    }
}

#[test]
fn test_milestone_11_2_zero_allocation_ring_buffer_circular_overwrite() {
    const CAPACITY: usize = 100;
    let mut ring_buffer: TraceRingBuffer<CAPACITY> = TraceRingBuffer::new();

    // Push 250 spans across multiple trace IDs
    for i in 0..250 {
        let ctx = TraceContext::new(
            i as u32,
            (i / 10) as u64,
            i as u32 + 100,
            1,
            1,
            (i as u64) + 1_000,
            1,
        );
        ring_buffer.push(TraceSpan::new("sim_phase", (i * 50) as u64, 10, ctx));
    }

    // Must be saturated at maximum capacity without heap growth
    assert_eq!(ring_buffer.len(), CAPACITY);
    assert!(ring_buffer.is_full());
    assert_eq!(ring_buffer.total_recorded(), 250);

    // Oldest surviving span should be span 150 (250 - 100)
    let first_popped = ring_buffer.pop().expect("buffer must not be empty");
    assert_eq!(first_popped.context.session_id, 150);
    assert_eq!(first_popped.context.trace_id, 1_150);
    assert_eq!(ring_buffer.len(), CAPACITY - 1);
}

#[test]
fn test_milestone_11_2_input_lag_latency_attribution_diagnostics() {
    let mut ring_buffer: TraceRingBuffer<32> = TraceRingBuffer::new();

    let fast_trace_id = 0x1111;
    let slow_trace_id = 0x9999;

    // Fast trace execution
    let fast_root = TraceContext::new(10, 100, 100, 1, 1, fast_trace_id, 1);
    ring_buffer.push(TraceSpan::new("ingress", 1_000, 100, fast_root));
    ring_buffer.push(TraceSpan::new(
        "spatial",
        1_100,
        300,
        fast_root.child_span(2),
    ));
    ring_buffer.push(TraceSpan::new(
        "egress",
        1_400,
        100,
        fast_root.child_span(3),
    ));

    // Stalled trace execution (e.g. hotspot density stall in spatial partitioning)
    let slow_root = TraceContext::new(20, 100, 200, 1, 1, slow_trace_id, 1);
    ring_buffer.push(TraceSpan::new("ingress", 2_000, 120, slow_root));
    ring_buffer.push(TraceSpan::new(
        "spatial_hotspot_stall",
        2_120,
        28_000,
        slow_root.child_span(2),
    ));
    ring_buffer.push(TraceSpan::new(
        "egress",
        30_120,
        150,
        slow_root.child_span(3),
    ));

    // Retrieve slow trace for root-cause diagnosis
    let mut slow_spans = [TraceSpan::new("", 0, 0, TraceContext::default()); 4];
    let count = ring_buffer.find_by_trace_id(slow_trace_id, &mut slow_spans);
    assert_eq!(count, 3);

    let total_slow_latency_micros: u32 =
        slow_spans[..count].iter().map(|s| s.duration_micros).sum();
    assert_eq!(total_slow_latency_micros, 120 + 28_000 + 150);

    // Identify offending hotspot stall
    let max_span = slow_spans[..count]
        .iter()
        .max_by_key(|s| s.duration_micros)
        .expect("spans exist");
    assert_eq!(max_span.name, "spatial_hotspot_stall");
    assert_eq!(max_span.duration_micros, 28_000);
}
