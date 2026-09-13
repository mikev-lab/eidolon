//! Low-latency UDP transport layer, register-width bitpacking, and sequenced channels.
//!
//! `eidolon-net` handles zero-copy bitstream serialization and client network sequencing
//! with hard memory bounds and zero dynamic allocations inside hot network paths.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod admission;
pub mod auth;
pub mod backpressure;
pub mod bitstream;
pub mod channel;
pub mod crypto;
pub mod error;
pub mod fuzz;
pub mod impairment;
pub mod packet;
pub mod pcap;
pub mod protocol;
pub mod queue;
pub mod quota;
pub mod version;
pub mod wire_accounting;

// Re-export primary types for ergonomic crate consumption.
pub use admission::{AdmissionConfig, AdmissionController, AdmissionMetrics, PriorityClass};
pub use auth::{
    compute_auth_cookie, compute_client_proof, verify_client_proof, ConnectChallengeRequest,
    ConnectChallengeResponse, ConnectFinalizeRequest, ConnectFinalizeResponse, ReplayWindow,
    SessionSecurityContext, CHALLENGE_REQ_LEN, CHALLENGE_RESP_LEN, FINALIZE_REQ_LEN,
    FINALIZE_RESP_LEN, NONCE_LEN,
};
pub use backpressure::{
    BackpressureCoordinator, BackpressureLevel, BackpressureMetrics, BoundedEgressQueue,
    BoundedIngressQueue, PrioritizedPacket,
};
pub use bitstream::{BitReader, BitWriter};
pub use channel::{
    IncomingReliablePacket, PendingReliablePacket, ReliableChannel, UnreliableSequencer,
};
pub use crypto::{constant_time_eq, hmac_sha256, sha256};
pub use error::{BitstreamError, NetError};
pub use fuzz::{FuzzStrategy, PacketFuzzGenerator};
pub use impairment::{
    DelayedPacket, FastPrng, ImpairmentConfig, ImpairmentMetrics, ImpairmentOutcome,
    NetworkImpairmentHarness,
};
pub use packet::{PacketHeader, PacketView};
pub use pcap::{PcapWriter, LINKTYPE_ETHERNET, PCAP_MAGIC_NUMBER};
pub use protocol::{
    ChannelType, PacketType, HEADER_SIZE, MAX_PACKET_SIZE, PROTOCOL_MAGIC, PROTOCOL_VERSION,
};
pub use queue::PacketRingBuffer;
pub use quota::{
    ConnectionMemoryAccountant, SocketRatePolicer, DEFAULT_MAX_BPS, DEFAULT_MAX_PPS,
    MAX_CONNECTION_MEMORY,
};
pub use version::{
    ProtocolFeatures, ProtocolNegotiator, MAX_SUPPORTED_VERSION, MIN_SUPPORTED_VERSION,
};
pub use wire_accounting::{BandwidthSummary, PhysicalFrameBreakdown, SpatialBandwidthProfile};
