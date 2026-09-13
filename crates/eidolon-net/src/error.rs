//! Error types for bitstream operations and network protocol handling.

use std::fmt;

/// Errors that can occur during bitstream reading or writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitstreamError {
    /// Attempted to write bits beyond the capacity of the buffer.
    BufferOverflow {
        /// Number of bits required for the operation.
        required_bits: usize,
        /// Number of bits remaining in the buffer.
        available_bits: usize,
    },
    /// Unexpected end of bitstream during reading.
    UnexpectedEof {
        /// Number of bits requested.
        requested_bits: usize,
        /// Number of bits remaining in the stream.
        remaining_bits: usize,
    },
    /// Variable-length integer encoding exceeded the maximum allowed byte length.
    InvalidVarint,
    /// Bit width specified is invalid for the requested integer type.
    InvalidBitWidth(usize),
}

impl fmt::Display for BitstreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BufferOverflow {
                required_bits,
                available_bits,
            } => write!(
                f,
                "Bitstream buffer overflow: required {required_bits} bits, but only {available_bits} bits available"
            ),
            Self::UnexpectedEof {
                requested_bits,
                remaining_bits,
            } => write!(
                f,
                "Unexpected EOF in bitstream: requested {requested_bits} bits, but only {remaining_bits} bits remain"
            ),
            Self::InvalidVarint => write!(f, "Invalid variable-length integer encoding"),
            Self::InvalidBitWidth(width) => write!(f, "Invalid bit width: {width}"),
        }
    }
}

impl std::error::Error for BitstreamError {}

/// High-level network protocol and transport errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    /// Bitstream-level serialization or deserialization error.
    Bitstream(BitstreamError),
    /// Invalid protocol magic bytes encountered in packet header.
    InvalidMagic {
        /// Expected magic bytes.
        expected: [u8; 2],
        /// Received magic bytes.
        received: [u8; 2],
    },
    /// Protocol version does not match expected version.
    UnsupportedVersion {
        /// Expected protocol version.
        expected: u16,
        /// Received protocol version.
        received: u16,
    },
    /// Packet channel identifier is unrecognized.
    InvalidChannel(u8),
    /// Packet payload length exceeds maximum transmission budget.
    PayloadTooLarge {
        /// Length claimed in packet header.
        length: usize,
        /// Maximum allowable length.
        max: usize,
    },
    /// Packet byte length is shorter than the minimum header length.
    TruncatedPacket {
        /// Expected minimum byte length.
        expected_len: usize,
        /// Actual byte length received.
        actual_len: usize,
    },
    /// Per-client queue is full (backpressure saturation).
    QueueFull,
    /// Sequence comparison or ACK bitfield is corrupted.
    CorruptedData,
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bitstream(err) => write!(f, "Bitstream error: {err}"),
            Self::InvalidMagic { expected, received } => write!(
                f,
                "Invalid protocol magic: expected {expected:?}, received {received:?}"
            ),
            Self::UnsupportedVersion { expected, received } => write!(
                f,
                "Unsupported protocol version: expected {expected}, received {received}"
            ),
            Self::InvalidChannel(channel) => write!(f, "Invalid channel identifier: {channel}"),
            Self::PayloadTooLarge { length, max } => write!(
                f,
                "Payload length {length} exceeds maximum allowable length of {max}"
            ),
            Self::TruncatedPacket {
                expected_len,
                actual_len,
            } => write!(
                f,
                "Truncated packet: expected at least {expected_len} bytes, received {actual_len} bytes"
            ),
            Self::QueueFull => write!(f, "Packet queue is saturated (backpressure limit reached)"),
            Self::CorruptedData => write!(f, "Corrupted packet or channel data"),
        }
    }
}

impl std::error::Error for NetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bitstream(err) => Some(err),
            _ => None,
        }
    }
}

impl From<BitstreamError> for NetError {
    #[inline]
    fn from(err: BitstreamError) -> Self {
        Self::Bitstream(err)
    }
}
