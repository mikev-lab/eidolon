//! Zero-copy packet framing, header serialization, and payload extraction.

use crate::error::NetError;
use crate::protocol::{
    ChannelType, PacketType, COMPACT_HEADER_SIZE, FLAG_COMPACT_HEADER, HEADER_SIZE,
    MAX_PACKET_SIZE, PROTOCOL_MAGIC, PROTOCOL_VERSION,
};

/// Fixed-size wire packet header containing sequence and sliding-window acknowledgment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    /// Wire protocol version.
    pub version: u16,
    /// Channel delivery guarantee.
    pub channel: ChannelType,
    /// Packet type and control flags.
    pub packet_type: PacketType,
    /// Outgoing packet sequence number.
    pub sequence: u16,
    /// Highest sequence number received from remote peer.
    pub ack: u16,
    /// 32-bit bitfield of received packets prior to `ack`.
    pub ack_bitfield: u32,
}

impl PacketHeader {
    /// Creates a new packet header with current protocol version.
    #[inline]
    pub fn new(
        channel: ChannelType,
        packet_type: PacketType,
        sequence: u16,
        ack: u16,
        ack_bitfield: u32,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            channel,
            packet_type,
            sequence,
            ack,
            ack_bitfield,
        }
    }

    /// Serializes the 12-byte header into the destination buffer.
    pub fn write_to(&self, out: &mut [u8]) -> Result<usize, NetError> {
        let actual_len = out.len();
        if actual_len < HEADER_SIZE {
            return Err(NetError::TruncatedPacket {
                expected_len: HEADER_SIZE,
                actual_len,
            });
        }

        let header_slice = out
            .get_mut(..HEADER_SIZE)
            .ok_or(NetError::TruncatedPacket {
                expected_len: HEADER_SIZE,
                actual_len,
            })?;

        let seq_bytes = self.sequence.to_be_bytes();
        let ack_bytes = self.ack.to_be_bytes();
        let ack_bit_bytes = self.ack_bitfield.to_be_bytes();

        let raw = [
            PROTOCOL_MAGIC[0],
            PROTOCOL_MAGIC[1],
            (self.version & 0xFF) as u8,
            (self.channel.as_u8() & 0x07) | (self.packet_type.as_nibble() << 4),
            seq_bytes[0],
            seq_bytes[1],
            ack_bytes[0],
            ack_bytes[1],
            ack_bit_bytes[0],
            ack_bit_bytes[1],
            ack_bit_bytes[2],
            ack_bit_bytes[3],
        ];
        header_slice.copy_from_slice(&raw);

        Ok(HEADER_SIZE)
    }

    /// Deserializes a packet header from an untrusted byte slice with strict bounds checking.
    pub fn read_from(slice: &[u8]) -> Result<(Self, usize), NetError> {
        if slice.len() < HEADER_SIZE {
            return Err(NetError::TruncatedPacket {
                expected_len: HEADER_SIZE,
                actual_len: slice.len(),
            });
        }

        let header_slice = slice.get(..HEADER_SIZE).ok_or(NetError::TruncatedPacket {
            expected_len: HEADER_SIZE,
            actual_len: slice.len(),
        })?;

        let magic = [header_slice[0], header_slice[1]];
        if magic != PROTOCOL_MAGIC {
            return Err(NetError::InvalidMagic {
                expected: PROTOCOL_MAGIC,
                received: magic,
            });
        }

        let version = header_slice[2] as u16;
        if version != PROTOCOL_VERSION {
            return Err(NetError::UnsupportedVersion {
                expected: PROTOCOL_VERSION,
                received: version,
            });
        }

        let channel = ChannelType::from_u8(header_slice[3] & 0x07)?;
        let packet_type = PacketType::from_nibble((header_slice[3] >> 4) & 0x0F)?;

        let sequence = u16::from_be_bytes([header_slice[4], header_slice[5]]);
        let ack = u16::from_be_bytes([header_slice[6], header_slice[7]]);
        let ack_bitfield = u32::from_be_bytes([
            header_slice[8],
            header_slice[9],
            header_slice[10],
            header_slice[11],
        ]);

        Ok((
            Self {
                version,
                channel,
                packet_type,
                sequence,
                ack,
                ack_bitfield,
            },
            HEADER_SIZE,
        ))
    }
}

/// Compact 6-byte wire packet header for high-frequency unreliable state updates.
///
/// Omits redundant ACK fields (`ack` and `ack_bitfield`), saving 6 bytes per movement packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactPacketHeader {
    /// Wire protocol version.
    pub version: u16,
    /// Channel delivery guarantee.
    pub channel: ChannelType,
    /// Packet type and control flags.
    pub packet_type: PacketType,
    /// Outgoing packet sequence number.
    pub sequence: u16,
}

impl CompactPacketHeader {
    /// Creates a new compact packet header.
    #[inline]
    pub fn new(channel: ChannelType, packet_type: PacketType, sequence: u16) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            channel,
            packet_type,
            sequence,
        }
    }

    /// Serializes the 6-byte compact header into the destination buffer.
    pub fn write_to(&self, out: &mut [u8]) -> Result<usize, NetError> {
        let actual_len = out.len();
        if actual_len < COMPACT_HEADER_SIZE {
            return Err(NetError::TruncatedPacket {
                expected_len: COMPACT_HEADER_SIZE,
                actual_len,
            });
        }

        let header_slice = out
            .get_mut(..COMPACT_HEADER_SIZE)
            .ok_or(NetError::TruncatedPacket {
                expected_len: COMPACT_HEADER_SIZE,
                actual_len,
            })?;

        let seq_bytes = self.sequence.to_be_bytes();
        let raw = [
            PROTOCOL_MAGIC[0],
            PROTOCOL_MAGIC[1],
            (self.version & 0xFF) as u8,
            (self.channel.as_u8() & 0x07)
                | (self.packet_type.as_nibble() << 4)
                | FLAG_COMPACT_HEADER,
            seq_bytes[0],
            seq_bytes[1],
        ];
        header_slice.copy_from_slice(&raw);

        Ok(COMPACT_HEADER_SIZE)
    }

    /// Deserializes a compact packet header from an untrusted byte slice.
    pub fn read_from(slice: &[u8]) -> Result<(Self, usize), NetError> {
        if slice.len() < COMPACT_HEADER_SIZE {
            return Err(NetError::TruncatedPacket {
                expected_len: COMPACT_HEADER_SIZE,
                actual_len: slice.len(),
            });
        }

        let header_slice = slice
            .get(..COMPACT_HEADER_SIZE)
            .ok_or(NetError::TruncatedPacket {
                expected_len: COMPACT_HEADER_SIZE,
                actual_len: slice.len(),
            })?;

        let magic = [header_slice[0], header_slice[1]];
        if magic != PROTOCOL_MAGIC {
            return Err(NetError::InvalidMagic {
                expected: PROTOCOL_MAGIC,
                received: magic,
            });
        }

        let version = header_slice[2] as u16;
        if version != PROTOCOL_VERSION {
            return Err(NetError::UnsupportedVersion {
                expected: PROTOCOL_VERSION,
                received: version,
            });
        }

        let channel = ChannelType::from_u8(header_slice[3] & 0x07)?;
        let packet_type = PacketType::from_nibble((header_slice[3] >> 4) & 0x0F)?;
        let sequence = u16::from_be_bytes([header_slice[4], header_slice[5]]);

        Ok((
            Self {
                version,
                channel,
                packet_type,
                sequence,
            },
            COMPACT_HEADER_SIZE,
        ))
    }
}

/// Unified polymorphic header representation supporting standard 12-byte and compact 6-byte variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderKind {
    /// Standard 12-byte header with sliding window ACKs.
    Standard(PacketHeader),
    /// Compact 6-byte header without ACKs.
    Compact(CompactPacketHeader),
}

impl HeaderKind {
    /// Deserializes either a standard or compact header automatically based on the compact flag bit.
    pub fn read_from(slice: &[u8]) -> Result<(Self, usize), NetError> {
        if slice.len() < COMPACT_HEADER_SIZE {
            return Err(NetError::TruncatedPacket {
                expected_len: COMPACT_HEADER_SIZE,
                actual_len: slice.len(),
            });
        }

        let flags = slice.get(3).copied().unwrap_or(0);
        if (flags & FLAG_COMPACT_HEADER) != 0 {
            let (compact, len) = CompactPacketHeader::read_from(slice)?;
            Ok((HeaderKind::Compact(compact), len))
        } else {
            let (standard, len) = PacketHeader::read_from(slice)?;
            Ok((HeaderKind::Standard(standard), len))
        }
    }

    /// Sequence number of the packet.
    #[inline]
    pub fn sequence(&self) -> u16 {
        match self {
            Self::Standard(h) => h.sequence,
            Self::Compact(h) => h.sequence,
        }
    }

    /// Channel delivery guarantee.
    #[inline]
    pub fn channel(&self) -> ChannelType {
        match self {
            Self::Standard(h) => h.channel,
            Self::Compact(h) => h.channel,
        }
    }

    /// Packet type.
    #[inline]
    pub fn packet_type(&self) -> PacketType {
        match self {
            Self::Standard(h) => h.packet_type,
            Self::Compact(h) => h.packet_type,
        }
    }
}

/// Zero-copy borrowing view over an incoming network packet with a polymorphic header.
#[derive(Debug, Clone, Copy)]
pub struct UnifiedPacketView<'a> {
    /// Parsed packet header variant.
    pub header: HeaderKind,
    /// Unparsed payload slice borrowing from the raw packet.
    pub payload: &'a [u8],
}

impl<'a> UnifiedPacketView<'a> {
    /// Parses an incoming network slice into a zero-copy unified packet view.
    pub fn from_bytes(slice: &'a [u8]) -> Result<Self, NetError> {
        if slice.len() > MAX_PACKET_SIZE {
            return Err(NetError::PayloadTooLarge {
                length: slice.len(),
                max: MAX_PACKET_SIZE,
            });
        }

        let (header, header_len) = HeaderKind::read_from(slice)?;
        let payload = slice.get(header_len..).unwrap_or(&[]);

        Ok(Self { header, payload })
    }
}

/// Zero-copy borrowing view over an incoming network packet.
#[derive(Debug, Clone, Copy)]
pub struct PacketView<'a> {
    /// Parsed packet header.
    pub header: PacketHeader,
    /// Unparsed payload slice borrowing from the raw packet.
    pub payload: &'a [u8],
}

impl<'a> PacketView<'a> {
    /// Parses an incoming network slice into a zero-copy packet view.
    pub fn from_bytes(slice: &'a [u8]) -> Result<Self, NetError> {
        if slice.len() > MAX_PACKET_SIZE {
            return Err(NetError::PayloadTooLarge {
                length: slice.len(),
                max: MAX_PACKET_SIZE,
            });
        }

        let (header, header_len) = PacketHeader::read_from(slice)?;
        let payload = slice.get(header_len..).unwrap_or(&[]);

        Ok(Self { header, payload })
    }

    /// Returns the total wire length of the packet in bytes.
    #[inline]
    pub fn wire_len(&self) -> usize {
        HEADER_SIZE + self.payload.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_header_roundtrip() {
        let header = PacketHeader::new(
            ChannelType::ReliableOrdered,
            PacketType::ReliableMessage,
            1234,
            1230,
            0b10101,
        );

        let mut buffer = [0u8; 16];
        let written = header.write_to(&mut buffer).expect("serialize header");
        assert_eq!(written, HEADER_SIZE);

        let (decoded, read_len) = PacketHeader::read_from(&buffer).expect("deserialize header");
        assert_eq!(read_len, HEADER_SIZE);
        assert_eq!(header, decoded);
    }

    #[test]
    fn test_packet_view_zero_copy() {
        let mut buffer = [0u8; 32];
        let header = PacketHeader::new(
            ChannelType::UnreliableSequenced,
            PacketType::StateUpdate,
            500,
            499,
            0xFFFFFFFF,
        );
        header.write_to(&mut buffer).expect("write header");

        let payload_data = b"hello world!";
        buffer[HEADER_SIZE..HEADER_SIZE + payload_data.len()].copy_from_slice(payload_data);

        let view = PacketView::from_bytes(&buffer[..HEADER_SIZE + payload_data.len()])
            .expect("parse packet view");
        assert_eq!(view.header.sequence, 500);
        assert_eq!(view.payload, payload_data);
    }

    #[test]
    fn test_malformed_header_rejections() {
        // Too short
        let short_buf = [0u8; 5];
        assert!(PacketHeader::read_from(&short_buf).is_err());

        // Invalid magic
        let mut bad_magic = [0u8; 12];
        bad_magic[0] = 0xAA;
        bad_magic[1] = 0xBB;
        assert!(PacketHeader::read_from(&bad_magic).is_err());

        // Unsupported version
        let mut bad_ver = [0u8; 12];
        bad_ver[0] = PROTOCOL_MAGIC[0];
        bad_ver[1] = PROTOCOL_MAGIC[1];
        bad_ver[2] = 99; // Version 99
        assert!(PacketHeader::read_from(&bad_ver).is_err());
    }

    #[test]
    fn test_compact_packet_header_roundtrip() {
        let compact = CompactPacketHeader::new(
            ChannelType::UnreliableSequenced,
            PacketType::StateUpdate,
            7777,
        );

        let mut buf = [0u8; 16];
        let written = compact.write_to(&mut buf).expect("write compact");
        assert_eq!(written, COMPACT_HEADER_SIZE);
        assert_eq!(written, 6);

        let (decoded, read_len) = CompactPacketHeader::read_from(&buf).expect("read compact");
        assert_eq!(read_len, COMPACT_HEADER_SIZE);
        assert_eq!(decoded.sequence, 7777);
        assert_eq!(decoded.channel, ChannelType::UnreliableSequenced);
        assert_eq!(decoded.packet_type, PacketType::StateUpdate);

        // HeaderKind polymorphic auto-detection
        let (kind, kind_len) = HeaderKind::read_from(&buf).expect("read kind");
        assert_eq!(kind_len, COMPACT_HEADER_SIZE);
        assert_eq!(kind.sequence(), 7777);
        match kind {
            HeaderKind::Compact(c) => assert_eq!(c.sequence, 7777),
            HeaderKind::Standard(_) => panic!("expected compact header"),
        }
    }

    #[test]
    fn test_unified_packet_view_polymorphic() {
        // Standard header packet
        let std_hdr = PacketHeader::new(
            ChannelType::ReliableOrdered,
            PacketType::ReliableMessage,
            100,
            90,
            0b111,
        );
        let mut std_buf = [0u8; 32];
        std_hdr.write_to(&mut std_buf).expect("write std");
        std_buf[HEADER_SIZE..HEADER_SIZE + 4].copy_from_slice(b"ping");
        let view_std =
            UnifiedPacketView::from_bytes(&std_buf[..HEADER_SIZE + 4]).expect("view std");
        assert_eq!(view_std.header.sequence(), 100);
        assert_eq!(view_std.payload, b"ping");

        // Compact header packet
        let cmp_hdr = CompactPacketHeader::new(
            ChannelType::UnreliableSequenced,
            PacketType::StateUpdate,
            200,
        );
        let mut cmp_buf = [0u8; 32];
        cmp_hdr.write_to(&mut cmp_buf).expect("write compact");
        cmp_buf[COMPACT_HEADER_SIZE..COMPACT_HEADER_SIZE + 4].copy_from_slice(b"move");
        let view_cmp = UnifiedPacketView::from_bytes(&cmp_buf[..COMPACT_HEADER_SIZE + 4])
            .expect("view compact");
        assert_eq!(view_cmp.header.sequence(), 200);
        assert_eq!(view_cmp.payload, b"move");
    }
}
