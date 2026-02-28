// sedan-bolt: Neo4j Bolt v4.1 protocol server for SedanDelivery.
//
// Standalone binary that embeds gql-core directly. Runs alongside Valkey
// in the same container as a background process.

pub mod packstream;
pub mod message;
pub mod protocol;
pub mod connection;
pub mod bridge;
