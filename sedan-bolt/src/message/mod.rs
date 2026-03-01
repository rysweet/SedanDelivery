//! Bolt message layer.
//!
//! Provides chunked framing ([`codec`]) and typed message encoding /
//! decoding ([`types`]) on top of the PackStream binary format.

pub mod codec;
pub mod types;
