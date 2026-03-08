//! sedandelivery-bolt: Neo4j Bolt v4.4 protocol server for SedanDelivery.
//!
//! A standalone binary that embeds gql-core directly.  Runs alongside Valkey
//! in the same container as a background process.
//!
//! # Modules
//!
//! - [`bridge`] -- Translates between gql-core types and Bolt values.
//! - [`connection`] -- TCP listener, handshake, and session loop.
//! - [`message`] -- Bolt message encoding / decoding and chunked framing.
//! - [`packstream`] -- PackStream binary codec (encoder, decoder, value types).
//! - [`protocol`] -- State machine, message dispatch, and tag constants.

pub mod bridge;
pub mod connection;
pub mod health;
pub mod message;
pub mod metrics;
pub mod packstream;
pub mod protocol;
pub mod tls;
pub mod websocket;
