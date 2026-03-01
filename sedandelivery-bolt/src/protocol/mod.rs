//! Bolt protocol state machine and message dispatch.
//!
//! Contains the session handler ([`handler`]), the connection state enum
//! ([`state`]), and shared tag constants ([`tags`]).

pub mod handler;
pub mod state;
pub mod tags;
