//! Physical Layer (L1) through Application Layer (L7) Wire Accounting Engine.
//!
//! Provides mathematically rigorous byte and bit accounting across all networking layers,
//! reconciling application payload footprints against physical Ethernet frames and cloud egress billing.

/// Layer 1 Physical framing overhead: 7-byte preamble + 1-byte SFD + 12-byte Interpacket Gap.
pub const L1_PHYSICAL_OVERHEAD: usize = 20;

/// Layer 2 Data Link overhead: 14-byte Ethernet II header + 4-byte Frame Check Sequence (FCS/CRC32).
pub const L2_ETHERNET_OVERHEAD: usize = 18;

/// Layer 3 Network overhead: Standard IPv4 header without options.
pub const L3_IPV4_OVERHEAD: usize = 20;

/// Layer 4 Transport overhead: Standard UDP header.
pub const L4_UDP_OVERHEAD: usize = 8;

/// Layer 7 Protocol overhead: Standard eidolon packet framing header.
pub const L7_PROTOCOL_OVERHEAD: usize = 12;

/// Total lower-layer network framing (L3 IPv4 + L4 UDP) billed by public cloud egress providers.
pub const CLOUD_EGRESS_FRAMING_OVERHEAD: usize = L3_IPV4_OVERHEAD + L4_UDP_OVERHEAD;

/// Total physical wire overhead across all lower layers (L1 + L2 + L3 + L4 + L7 header).
pub const TOTAL_PHYSICAL_OVERHEAD: usize = L1_PHYSICAL_OVERHEAD
    + L2_ETHERNET_OVERHEAD
    + L3_IPV4_OVERHEAD
    + L4_UDP_OVERHEAD
    + L7_PROTOCOL_OVERHEAD;

/// Detailed Layer 1 to Layer 7 byte breakdown for an individual packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalFrameBreakdown {
    /// Layer 1 Physical bits: Preamble, Start Frame Delimiter, Interpacket Gap (20 bytes).
    pub l1_bytes: usize,
    /// Layer 2 Data Link: Ethernet MAC headers and CRC32 FCS (18 bytes).
    pub l2_bytes: usize,
    /// Layer 3 Network: IPv4 header (20 bytes).
    pub l3_bytes: usize,
    /// Layer 4 Transport: UDP header (8 bytes).
    pub l4_bytes: usize,
    /// Layer 7 Protocol: eidolon packet sequence and channel header (12 bytes).
    pub l7_header_bytes: usize,
    /// Application payload bytes (quantized transforms, kinematic deltas, actions).
    pub payload_bytes: usize,
}

impl PhysicalFrameBreakdown {
    /// Computes the exact byte breakdown for an application payload of the specified length.
    pub const fn compute(payload_bytes: usize) -> Self {
        Self {
            l1_bytes: L1_PHYSICAL_OVERHEAD,
            l2_bytes: L2_ETHERNET_OVERHEAD,
            l3_bytes: L3_IPV4_OVERHEAD,
            l4_bytes: L4_UDP_OVERHEAD,
            l7_header_bytes: L7_PROTOCOL_OVERHEAD,
            payload_bytes,
        }
    }

    /// Total bytes physically transmitted across the wire/medium (L1 through L7).
    #[inline]
    pub const fn total_physical_bytes(&self) -> usize {
        self.l1_bytes
            + self.l2_bytes
            + self.l3_bytes
            + self.l4_bytes
            + self.l7_header_bytes
            + self.payload_bytes
    }

    /// Total bytes billed by cloud egress providers (IPv4 packet: L3 + L4 + L7 header + payload).
    #[inline]
    pub const fn cloud_egress_bytes(&self) -> usize {
        self.l3_bytes + self.l4_bytes + self.l7_header_bytes + self.payload_bytes
    }

    /// Total application-level payload efficiency (payload bytes / physical wire bytes).
    #[inline]
    pub fn wire_efficiency_ratio(&self) -> f64 {
        let physical = self.total_physical_bytes();
        if physical == 0 {
            0.0
        } else {
            self.payload_bytes as f64 / physical as f64
        }
    }
}

/// Simulated spatial frequency entity distribution profile for a client.
#[derive(Debug, Clone, Copy)]
pub struct SpatialBandwidthProfile {
    /// Number of entities in the Immediate Tier (<10m, updated at immediate_hz).
    pub immediate_entities: usize,
    /// Update frequency for Immediate Tier entities in Hertz (e.g. 10.0 Hz).
    pub immediate_hz: f64,
    /// Number of entities in the Mid Tier (10m - 50m, updated at mid_hz).
    pub mid_entities: usize,
    /// Update frequency for Mid Tier entities in Hertz (e.g. 2.0 Hz).
    pub mid_hz: f64,
    /// Number of entities in the Horizon Tier (>50m, event-only / 0.1 Hz).
    pub horizon_entities: usize,
    /// Update frequency for Horizon Tier entities in Hertz (e.g. 0.1 Hz).
    pub horizon_hz: f64,
    /// Ratio of potential transform updates eliminated by intent-based dead reckoning extrapolation (0.0 to 1.0).
    pub dead_reckoning_suppression_ratio: f64,
    /// Average size of an entity kinematic update payload (e.g. 7 bytes for transform).
    pub payload_bytes_per_entity: usize,
}

impl Default for SpatialBandwidthProfile {
    fn default() -> Self {
        Self {
            immediate_entities: 5,
            immediate_hz: 10.0,
            mid_entities: 15,
            mid_hz: 2.0,
            horizon_entities: 30,
            horizon_hz: 0.1,
            dead_reckoning_suppression_ratio: 0.60, // 60% eliminated via constant-velocity extrapolation
            payload_bytes_per_entity: 7,            // 7-byte quantized transform
        }
    }
}

/// Aggregated bandwidth metrics under a spatial entity profile.
#[derive(Debug, Clone, Copy)]
pub struct BandwidthSummary {
    /// Total packets sent per second to the client.
    pub packets_per_second: f64,
    /// Total application payload bytes per second.
    pub payload_bytes_per_sec: f64,
    /// Total cloud egress bytes per second (IP layer and up).
    pub cloud_egress_bytes_per_sec: f64,
    /// Total physical wire bytes per second (including Ethernet, preamble, and IFG).
    pub physical_wire_bytes_per_sec: f64,
}

impl SpatialBandwidthProfile {
    /// Computes client bandwidth consumption assuming per-packet coalescing (e.g. 20 Hz tick loop).
    ///
    /// At 20 Hz, updates maturing in a tick are batched into a single UDP datagram where feasible,
    /// incorporating intent-based dead reckoning to suppress constant-velocity transmissions.
    pub fn compute_coalesced_bandwidth(&self, server_hz: f64) -> BandwidthSummary {
        // Raw candidate updates across all spatial frequency tiers
        let raw_updates_per_sec = (self.immediate_entities as f64 * self.immediate_hz)
            + (self.mid_entities as f64 * self.mid_hz)
            + (self.horizon_entities as f64 * self.horizon_hz);

        // Intent-based dead reckoning: only transmit when trajectory/heading changes
        let unsuppressed_ratio = (1.0 - self.dead_reckoning_suppression_ratio).clamp(0.0, 1.0);
        let updates_per_sec = raw_updates_per_sec * unsuppressed_ratio;

        let payload_per_sec = updates_per_sec * self.payload_bytes_per_entity as f64;

        // Number of egress UDP packets per second bounded by server tick rate
        let packets_per_sec = if updates_per_sec < server_hz {
            updates_per_sec
        } else {
            server_hz
        };

        // When updates are batched into packets at server_hz:
        // Each packet carries (L3 + L4 + L7_header) framing = 40 bytes
        let cloud_overhead_per_sec =
            packets_per_sec * (CLOUD_EGRESS_FRAMING_OVERHEAD + L7_PROTOCOL_OVERHEAD) as f64;
        let cloud_egress_bytes_per_sec = payload_per_sec + cloud_overhead_per_sec;

        // Physical wire adds L1 (20B) and L2 (18B) framing = 38 bytes per packet
        let physical_overhead_per_sec =
            packets_per_sec * (L1_PHYSICAL_OVERHEAD + L2_ETHERNET_OVERHEAD) as f64;
        let physical_wire_bytes_per_sec = cloud_egress_bytes_per_sec + physical_overhead_per_sec;

        BandwidthSummary {
            packets_per_second: packets_per_sec,
            payload_bytes_per_sec: payload_per_sec,
            cloud_egress_bytes_per_sec,
            physical_wire_bytes_per_sec,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_physical_frame_breakdown_constants() {
        let breakdown = PhysicalFrameBreakdown::compute(7); // 7-byte quantized transform

        assert_eq!(breakdown.l1_bytes, 20);
        assert_eq!(breakdown.l2_bytes, 18);
        assert_eq!(breakdown.l3_bytes, 20);
        assert_eq!(breakdown.l4_bytes, 8);
        assert_eq!(breakdown.l7_header_bytes, 12);
        assert_eq!(breakdown.payload_bytes, 7);

        // Physical wire: 20 + 18 + 20 + 8 + 12 + 7 = 85 bytes
        assert_eq!(breakdown.total_physical_bytes(), 85);

        // Cloud egress: 20 + 8 + 12 + 7 = 47 bytes
        assert_eq!(breakdown.cloud_egress_bytes(), 47);
    }

    #[test]
    fn test_sub_1_2kb_wire_budget_under_standard_profile() {
        let profile = SpatialBandwidthProfile::default();
        let summary = profile.compute_coalesced_bandwidth(20.0);

        // Cloud egress must be strictly < 1,200 bytes/second (1.2 KB/s)
        assert!(
            summary.cloud_egress_bytes_per_sec < 1_200.0,
            "Cloud egress ({:.2} B/s) must be strictly < 1,200 B/s",
            summary.cloud_egress_bytes_per_sec
        );

        // Even with physical Layer 1 preamble and Layer 2 Ethernet CRC, wire footprint remains bounded
        assert!(
            summary.physical_wire_bytes_per_sec < 2_000.0,
            "Physical wire ({:.2} B/s) must remain bounded",
            summary.physical_wire_bytes_per_sec
        );

        assert!(summary.packets_per_second <= 20.0);
    }
}
