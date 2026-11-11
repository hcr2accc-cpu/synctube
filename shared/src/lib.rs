//! Shared protocol types, limits and pure sync math for SyncTube.
//!
//! This crate performs no I/O.

pub mod limits;
pub mod protocol;
pub mod sync;
pub mod youtube;

pub use protocol::*;
