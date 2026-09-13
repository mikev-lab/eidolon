//! Universal C ABI dynamic and static library for game engine integration.
//!
//! Exposes an unmanaged C ABI interface for Godot, Unreal Engine, Unity,
//! and proprietary in-house C/C++ game engines with zero third-party dependencies.

#![allow(unsafe_code)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![warn(missing_docs)]

pub mod c_api;
pub mod types;

pub use c_api::*;
pub use types::*;
