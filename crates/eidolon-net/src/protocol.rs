//! Packet protocol definitions, magic headers, and channel guarantees.

use crate::error::NetError;

/// Protocol identifier magic constant ('E', 'I' -> 0x45, 0x49).
pub const PROTOCOL_MAGIC: [u8; 2] = [0x45, 0x49];

/// Current wire protocol version.
pub const PROTOCOL_VERSION: u16 = 1;

/// Maximum Transmission Unit (MTU) safe packet payload size in bytes.
///
/// Bounded to 1200 bytes to avoid IPv4/IPv6 fragmentation across public internet paths.
pub const MAX_PACKET_SIZE: usize = 1200;

/// Fixed byte length of the standard packet header.
pub const HEADER_SIZE: usize = 12;

/// Channel delivery guarantees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ChannelType {
    /// High-frequency sequenced unreliable channel (e.g. transforms, combat ticks).
    UnreliableSequenced = 0,
    /// Ordered reliable channel with acknowledgment (e.g. inventory, chat, party state).
    ReliableOrdered = 1,
}

impl ChannelType {
    /// Parses a channel type from a raw byte value.
    #[inline]
    pub fn from_u8(val: u8) -> Result<Self, NetError> {
        match val {
            0 => Ok(Self::UnreliableSequenced),
            1 => Ok(Self::ReliableOrdered),
            other => Err(NetError::InvalidChannel(other)),
        }
    }

    /// Returns the raw byte representation of the channel type.
    #[inline]
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }
}

/// High-level packet purpose and control flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    /// Keep-alive ping / heartbeat packet.
    Heartbeat = 0,
    /// Authoritative or predicted world state delta.
    StateUpdate = 1,
    /// Reliable message payload requiring guaranteed delivery.
    ReliableMessage = 2,
    /// Acknowledgment-only packet carrying sequence confirmation.
    AckOnly = 3,
    /// Orderly connection termination handshake.
    Disconnect = 4,
}

impl PacketType {
    /// Parses a packet type from a 4-bit nibble value.
    #[inline]
    pub fn from_nibble(val: u8) -> Result<Self, NetError> {
        match val & 0x0F {
            0 => Ok(Self::Heartbeat),
            1 => Ok(Self::StateUpdate),
            2 => Ok(Self::ReliableMessage),
            3 => Ok(Self::AckOnly),
            4 => Ok(Self::Disconnect),
            _ => Err(NetError::CorruptedData),
        }
    }

    /// Returns the 4-bit nibble representation of the packet type.
    #[inline]
    pub fn as_nibble(&self) -> u8 {
        *self as u8
    }
}
