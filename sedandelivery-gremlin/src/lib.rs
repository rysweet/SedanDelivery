//! sedandelivery-gremlin — Gremlin WebSocket server for SedanDelivery.
//!
//! Accepts Gremlin bytecode over WebSocket (port 8182) and translates
//! traversals into gql-core query execution. Health endpoint on port 8183.

pub mod bridge;
pub mod connection;
pub mod health;
pub mod protocol;
