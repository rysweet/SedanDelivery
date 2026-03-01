//! Bridge between the Bolt protocol and gql-core.
//!
//! Translates Bolt values into gql-core `Value` types, executes Cypher
//! queries via gql-core, and converts results back to Bolt records.

pub mod admin;
pub mod errors;
pub mod executor;
pub mod types;
