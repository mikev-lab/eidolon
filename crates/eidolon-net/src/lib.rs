//! Low-latency UDP transport layer, register-width bitpacking, and sequenced channels.
//!
//! `eidolon-net` handles zero-copy bitstream serialization and client network sequencing
//! with hard memory bounds and zero dynamic allocations inside hot network paths.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod bitstream;
pub mod channel;
pub mod error;
pub mod packet;
pub mod protocol;
pub mod queue;

// Re-export primary types for ergonomic crate consumption.
pub use bitstream::{BitReader, BitWriter};
pub use channel::{
    IncomingReliablePacket, PendingReliablePacket, ReliableChannel, UnreliableSequencer,
};
pub use error::{BitstreamError, NetError};
pub use packet::{PacketHeader, PacketView};
pub use protocol::{
    ChannelType, PacketType, HEADER_SIZE, MAX_PACKET_SIZE, PROTOCOL_MAGIC, PROTOCOL_VERSION,
};
pub use queue::PacketRingBuffer;
