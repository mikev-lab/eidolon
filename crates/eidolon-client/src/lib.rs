//! Native game client SDK, UDP connection handshake, kinematic dead reckoning, and smooth interpolation.
//!
//! `eidolon-client` handles client-side UDP networking, cryptographic authentication,
//! and deterministic dead reckoning extrapolation to provide smooth 60/120/144 FPS rendering
//! between 20 Hz authoritative server ticks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod client;
pub mod config;
pub mod event;
pub mod world_view;

pub use client::{ClientError, EidolonClient};
pub use config::ClientConfig;
pub use event::ClientEvent;
pub use world_view::{ClientTransform, ClientWorldView, RemoteEntity};
