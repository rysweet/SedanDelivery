//! Gremlin TCK test infrastructure for SedanDelivery.
//!
//! These tests exercise the gql-core query pipeline (parse → plan → execute)
//! against the standard TinkerPop "modern" graph, verifying behaviour
//! equivalent to the Gremlin TCK Gherkin feature files.
//!
//! The downloaded `.feature` files under `features/` serve as reference
//! material; the Rust tests below are the executable specification.

pub mod modern_graph;

#[cfg(test)]
mod tck_tests;
