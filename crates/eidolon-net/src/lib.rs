//! Low-latency UDP transport layer, register-width bitpacking, and sequenced channels.
//!
//! `eidolon-net` handles zero-copy bitstream serialization and client network sequencing
//! with hard memory bounds and zero dynamic allocations inside hot network paths.

#![deny(unsafe_code)]
#![warn(missing_docs)]

/// Register-width bitstream reader and writer primitives.
pub mod bitstream {
    /// Bounded bitstream writer operating directly on memory slices.
    #[derive(Debug)]
    pub struct BitWriter<'a> {
        buffer: &'a mut [u8],
        bit_offset: usize,
    }

    impl<'a> BitWriter<'a> {
        /// Creates a new `BitWriter` wrapping a mutable byte slice.
        #[inline]
        pub fn new(buffer: &'a mut [u8]) -> Self {
            Self {
                buffer,
                bit_offset: 0,
            }
        }

        /// Returns the current bit position within the stream.
        #[inline]
        pub fn bit_offset(&self) -> usize {
            self.bit_offset
        }

        /// Returns the number of fully or partially written bytes.
        #[inline]
        pub fn byte_len(&self) -> usize {
            self.bit_offset.div_ceil(8)
        }

        /// Returns the total capacity of the underlying byte slice in bytes.
        #[inline]
        pub fn buffer_capacity(&self) -> usize {
            self.buffer.len()
        }
    }
}

/// Packet framing, protocol magic, and opcode definitions.
pub mod protocol {
    /// Protocol identifier magic constant ('E', 'I', 'D', 'O').
    pub const PROTOCOL_MAGIC: [u8; 4] = [0x45, 0x49, 0x44, 0x4F];

    /// Protocol version identifier.
    pub const PROTOCOL_VERSION: u16 = 1;

    /// Channel delivery guarantees.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ChannelType {
        /// High-frequency sequenced unreliable channel (e.g. transforms, combat ticks).
        UnreliableSequenced = 0,
        /// Ordered reliable channel with acknowledgment (e.g. inventory, chat, party state).
        ReliableOrdered = 1,
    }
}

/// Packet sequencing and sliding window acknowledgments.
pub mod channel {
    /// Sequencer tracking packet delivery and out-of-order detection.
    #[derive(Debug, Default)]
    pub struct PacketSequencer {
        next_outgoing_sequence: u16,
        last_received_sequence: u16,
    }

    impl PacketSequencer {
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
        pub fn last_received_sequence(&self) -> u16 {
            self.last_received_sequence
        }

        /// Records a newly received packet sequence number if it is newer than previous packets.
        pub fn record_received_sequence(&mut self, seq: u16) {
            if Self::is_sequence_newer(seq, self.last_received_sequence) {
                self.last_received_sequence = seq;
            }
        }

        /// Computes whether `seq_a` is newer than `seq_b` handling circular 16-bit rollover.
        #[inline]
        pub fn is_sequence_newer(seq_a: u16, seq_b: u16) -> bool {
            ((seq_a.wrapping_sub(seq_b)) as i16) > 0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::bitstream::BitWriter;
    use super::channel::PacketSequencer;
    use super::protocol::{PROTOCOL_MAGIC, PROTOCOL_VERSION};

    #[test]
    fn test_protocol_constants() {
        assert_eq!(PROTOCOL_MAGIC, [0x45, 0x49, 0x44, 0x4F]);
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    #[test]
    fn test_bit_writer_initial_state() {
        let mut buffer = [0u8; 64];
        let writer = BitWriter::new(&mut buffer);
        assert_eq!(writer.bit_offset(), 0);
        assert_eq!(writer.byte_len(), 0);
    }

    #[test]
    fn test_sequence_newer_comparison() {
        assert!(PacketSequencer::is_sequence_newer(10, 5));
        assert!(!PacketSequencer::is_sequence_newer(5, 10));
        // Circular rollover check
        assert!(PacketSequencer::is_sequence_newer(0, 65535));
        assert!(!PacketSequencer::is_sequence_newer(65535, 0));
    }
}
