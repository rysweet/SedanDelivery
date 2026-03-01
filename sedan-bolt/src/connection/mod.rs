//! TCP connection management.
//!
//! Handles listening for incoming connections ([`listener`]), performing the
//! Bolt version-negotiation handshake ([`handshake`]), and running the
//! per-connection message loop ([`session`]).

pub mod handshake;
pub mod listener;
pub mod session;
