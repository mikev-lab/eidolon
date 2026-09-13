//! Sequenced channels and sliding-window selective acknowledgments.
//!
//! Provides both unreliable sequenced streams for movement transforms
//! and ordered reliable streams with zero heap allocations.

use crate::error::NetError;

/// Maximum payload size stored per pending reliable packet.
pub const MAX_RELIABLE_PAYLOAD: usize = 256;

/// Retransmission timeout in simulation ticks.
pub const DEFAULT_RETRY_TICKS: u32 = 4;

/// Sequencer tracking packet delivery and out-of-order detection for unreliable streams.
#[derive(Debug, Default, Clone)]
pub struct UnreliableSequencer {
    next_outgoing_sequence: u16,
    highest_received_sequence: u16,
    received_bitfield: u32,
    has_received_any: bool,
}

impl UnreliableSequencer {
    /// Creates a new packet sequencer with zeroed sequence counters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Increments and returns the next outgoing packet sequence number.
    #[inline]
    pub fn next_sequence(&mut self) -> u16 {
        let seq = self.next_outgoing_sequence;
        self.next_outgoing_sequence = self.next_outgoing_sequence.wrapping_add(1);
        seq
    }

    /// Returns the highest sequence number received from the remote peer.
    #[inline]
    pub fn highest_received_sequence(&self) -> u16 {
        self.highest_received_sequence
    }

    /// Returns the 32-bit history bitfield of packets received prior to highest sequence.
    #[inline]
    pub fn received_bitfield(&self) -> u32 {
        self.received_bitfield
    }

    /// Returns current ACK header parameters `(highest_seq, ack_bitfield)` to include in outgoing packets.
    #[inline]
    pub fn ack_data(&self) -> (u16, u32) {
        (self.highest_received_sequence, self.received_bitfield)
    }

    /// Computes whether `seq_a` is newer than `seq_b` handling circular 16-bit rollover.
    #[inline]
    pub fn is_sequence_newer(seq_a: u16, seq_b: u16) -> bool {
        ((seq_a.wrapping_sub(seq_b)) as i16) > 0
    }

    /// Ingests an incoming packet sequence number.
    ///
    /// Returns `true` if the packet is strictly newer than all previously received packets,
    /// or `false` if it is an out-of-order or duplicate packet.
    pub fn process_incoming_sequence(&mut self, seq: u16) -> bool {
        if !self.has_received_any {
            self.has_received_any = true;
            self.highest_received_sequence = seq;
            self.received_bitfield = 0;
            return true;
        }

        if Self::is_sequence_newer(seq, self.highest_received_sequence) {
            let diff = seq.wrapping_sub(self.highest_received_sequence) as usize;
            if diff <= 32 {
                self.received_bitfield = (self.received_bitfield << diff) | (1 << (diff - 1));
            } else {
                self.received_bitfield = 0;
            }
            self.highest_received_sequence = seq;
            true
        } else {
            let diff = self.highest_received_sequence.wrapping_sub(seq) as usize;
            if diff > 0 && diff <= 32 {
                self.received_bitfield |= 1 << (diff - 1);
            }
            false
        }
    }
}

/// Buffer entry for pending unacknowledged reliable packets.
#[derive(Debug, Clone, Copy)]
pub struct PendingReliablePacket {
    /// Outgoing sequence number assigned to this packet.
    pub sequence: u16,
    /// Stored packet payload data.
    pub payload: [u8; MAX_RELIABLE_PAYLOAD],
    /// Length of active payload data in bytes.
    pub len: usize,
    /// Countdown ticks until next retransmission attempt.
    pub retry_countdown: u32,
}

/// Buffer entry for out-of-order received reliable packets.
#[derive(Debug, Clone, Copy)]
pub struct IncomingReliablePacket {
    /// Sequence number of the received packet.
    pub sequence: u16,
    /// Stored payload data.
    pub payload: [u8; MAX_RELIABLE_PAYLOAD],
    /// Length of active payload data in bytes.
    pub len: usize,
}

/// Sliding-window reliable ordered channel supporting selective acknowledgment.
#[derive(Debug)]
pub struct ReliableChannel<const PENDING_CAP: usize, const ORDERED_CAP: usize> {
    next_outgoing_seq: u16,
    expected_incoming_seq: u16,
    pending_packets: [Option<PendingReliablePacket>; PENDING_CAP],
    reorder_buffer: [Option<IncomingReliablePacket>; ORDERED_CAP],
}

impl<const PENDING_CAP: usize, const ORDERED_CAP: usize> ReliableChannel<PENDING_CAP, ORDERED_CAP> {
    /// Creates a new `ReliableChannel` with empty retransmission and reorder buffers.
    pub fn new() -> Self {
        Self {
            next_outgoing_seq: 0,
            expected_incoming_seq: 0,
            pending_packets: std::array::from_fn(|_| None),
            reorder_buffer: std::array::from_fn(|_| None),
        }
    }

    /// Queues a reliable payload for delivery.
    ///
    /// Assigns a sequence number and stores in the retransmission buffer.
    pub fn queue_reliable_message(&mut self, data: &[u8]) -> Result<u16, NetError> {
        if data.len() > MAX_RELIABLE_PAYLOAD {
            return Err(NetError::PayloadTooLarge {
                length: data.len(),
                max: MAX_RELIABLE_PAYLOAD,
            });
        }

        // Find an empty pending slot
        let slot = self
            .pending_packets
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(NetError::QueueFull)?;

        let seq = self.next_outgoing_seq;
        self.next_outgoing_seq = self.next_outgoing_seq.wrapping_add(1);

        let mut payload = [0u8; MAX_RELIABLE_PAYLOAD];
        payload
            .get_mut(..data.len())
            .ok_or(NetError::QueueFull)?
            .copy_from_slice(data);

        *slot = Some(PendingReliablePacket {
            sequence: seq,
            payload,
            len: data.len(),
            retry_countdown: DEFAULT_RETRY_TICKS,
        });

        Ok(seq)
    }

    /// Ingests remote acknowledgment data (`ack` and `ack_bitfield`), retiring confirmed packets.
    pub fn process_remote_ack(&mut self, ack: u16, ack_bitfield: u32) {
        for slot in self.pending_packets.iter_mut() {
            if let Some(pending) = slot {
                let p_seq = pending.sequence;
                if p_seq == ack {
                    *slot = None;
                } else if UnreliableSequencer::is_sequence_newer(ack, p_seq) {
                    let diff = ack.wrapping_sub(p_seq) as usize;
                    if diff > 0 && diff <= 32 {
                        let is_acked = ((ack_bitfield >> (diff - 1)) & 1) != 0;
                        if is_acked {
                            *slot = None;
                        }
                    }
                }
            }
        }
    }

    /// Ingests an incoming reliable packet.
    ///
    /// Returns `Ok(Some((data, len)))` if the packet is the next expected in sequence,
    /// or `Ok(None)` if it was buffered or discarded as a duplicate.
    pub fn receive_reliable_packet(
        &mut self,
        seq: u16,
        data: &[u8],
    ) -> Result<Option<([u8; MAX_RELIABLE_PAYLOAD], usize)>, NetError> {
        if data.len() > MAX_RELIABLE_PAYLOAD {
            return Err(NetError::PayloadTooLarge {
                length: data.len(),
                max: MAX_RELIABLE_PAYLOAD,
            });
        }

        if seq == self.expected_incoming_seq {
            let mut payload = [0u8; MAX_RELIABLE_PAYLOAD];
            payload
                .get_mut(..data.len())
                .ok_or(NetError::CorruptedData)?
                .copy_from_slice(data);
            self.expected_incoming_seq = self.expected_incoming_seq.wrapping_add(1);
            Ok(Some((payload, data.len())))
        } else if UnreliableSequencer::is_sequence_newer(seq, self.expected_incoming_seq) {
            // Buffer future out-of-order packet
            for slot in self.reorder_buffer.iter_mut() {
                if slot.is_none() {
                    let mut payload = [0u8; MAX_RELIABLE_PAYLOAD];
                    payload
                        .get_mut(..data.len())
                        .ok_or(NetError::CorruptedData)?
                        .copy_from_slice(data);
                    *slot = Some(IncomingReliablePacket {
                        sequence: seq,
                        payload,
                        len: data.len(),
                    });
                    return Ok(None);
                }
            }
            Err(NetError::QueueFull)
        } else {
            // Older sequence: duplicate packet, ignore safely
            Ok(None)
        }
    }

    /// Drains any subsequent in-order packets from the reorder buffer.
    pub fn drain_next_ordered_packet(&mut self) -> Option<([u8; MAX_RELIABLE_PAYLOAD], usize)> {
        for slot in self.reorder_buffer.iter_mut() {
            if let Some(incoming) = slot {
                if incoming.sequence == self.expected_incoming_seq {
                    let payload = incoming.payload;
                    let len = incoming.len;
                    *slot = None;
                    self.expected_incoming_seq = self.expected_incoming_seq.wrapping_add(1);
                    return Some((payload, len));
                }
            }
        }
        None
    }

    /// Ticks retransmission timers and returns any packets needing retransmission.
    pub fn check_retransmissions<F>(&mut self, mut on_retry: F)
    where
        F: FnMut(u16, &[u8]),
    {
        for pending in self.pending_packets.iter_mut().flatten() {
            if pending.retry_countdown == 0 {
                pending.retry_countdown = DEFAULT_RETRY_TICKS;
                if let Some(payload_slice) = pending.payload.get(..pending.len) {
                    on_retry(pending.sequence, payload_slice);
                }
            } else {
                pending.retry_countdown = pending.retry_countdown.saturating_sub(1);
            }
        }
    }

    /// Returns the number of currently pending unacknowledged packets.
    pub fn pending_count(&self) -> usize {
        self.pending_packets.iter().filter(|s| s.is_some()).count()
    }
}

impl<const PENDING_CAP: usize, const ORDERED_CAP: usize> Default
    for ReliableChannel<PENDING_CAP, ORDERED_CAP>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unreliable_sequencer_in_order() {
        let mut seq = UnreliableSequencer::new();
        assert!(seq.process_incoming_sequence(0));
        assert!(seq.process_incoming_sequence(1));
        assert!(seq.process_incoming_sequence(2));
        assert_eq!(seq.highest_received_sequence(), 2);
    }

    #[test]
    fn test_unreliable_sequencer_out_of_order_and_duplicates() {
        let mut seq = UnreliableSequencer::new();
        assert!(seq.process_incoming_sequence(10));
        // Stale older sequence
        assert!(!seq.process_incoming_sequence(5));
        // Duplicate sequence
        assert!(!seq.process_incoming_sequence(10));
        // Newer sequence
        assert!(seq.process_incoming_sequence(11));
        assert_eq!(seq.highest_received_sequence(), 11);
    }

    #[test]
    fn test_unreliable_sequencer_rollover() {
        assert!(UnreliableSequencer::is_sequence_newer(0, 65535));
        assert!(!UnreliableSequencer::is_sequence_newer(65535, 0));
    }

    #[test]
    fn test_reliable_channel_ack_retirement() {
        let mut channel = ReliableChannel::<16, 16>::new();
        let _s0 = channel.queue_reliable_message(b"msg 0").expect("queue 0");
        let s1 = channel.queue_reliable_message(b"msg 1").expect("queue 1");
        assert_eq!(channel.pending_count(), 2);

        // Ack message 1, bitfield includes message 0
        channel.process_remote_ack(s1, 0b1);
        assert_eq!(channel.pending_count(), 0);
    }

    #[test]
    fn test_reliable_channel_in_order_reassembly() {
        let mut channel = ReliableChannel::<16, 16>::new();

        // Message 1 arrives before Message 0
        let res1 = channel
            .receive_reliable_packet(1, b"world")
            .expect("receive 1");
        assert!(res1.is_none());

        // Message 0 arrives
        let res0 = channel
            .receive_reliable_packet(0, b"hello ")
            .expect("receive 0");
        assert!(res0.is_some());
        let (p0, len0) = res0.unwrap();
        assert_eq!(&p0[..len0], b"hello ");

        // Drain Message 1 from reorder buffer
        let drained = channel.drain_next_ordered_packet();
        assert!(drained.is_some());
        let (p1, len1) = drained.unwrap();
        assert_eq!(&p1[..len1], b"world");
    }
}
