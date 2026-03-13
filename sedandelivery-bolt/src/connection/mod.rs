//! TCP connection management.
//!
//! Handles performing the Bolt version-negotiation handshake ([`handshake`]),
//! and running the per-connection message loop ([`session`]).

pub mod handshake;
pub mod per_ip_guard;
pub mod session;
