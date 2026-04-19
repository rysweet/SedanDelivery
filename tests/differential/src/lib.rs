//! Differential testing harness for SedanDelivery's three query engines.
//!
//! This crate asserts result parity between [`cypher_core`] (openCypher),
//! [`gql_engine`] (GQL), and [`gremlin_core`] (Gremlin) on a shared graph
//! fixture. It catches divergence bugs where one engine returns a different
//! result than the others for semantically equivalent queries.
//!
//! # Layout
//!
//! - [`fixture`]: builds a curated graph (~50 nodes, ~100 edges) shared by
//!   every parity case.
//! - [`runner`]: runs a query through each engine and produces a canonical
//!   (sorted, normalized) row representation suitable for `assert_eq!`.
//! - [`cases`]: the catalog of [`ParityCase`]s — each is a triple of
//!   semantically equivalent queries (Cypher / GQL / Gremlin).
//!
//! See `docs/DIFFERENTIAL_TESTING.md` for guidance on adding new cases and
//! the list of documented divergences.

pub mod cases;
pub mod fixture;
pub mod runner;

pub use cases::{DivergenceMode, ParityCase};
pub use fixture::{build_fixture, GRAPH};
pub use runner::{run_case, CaseOutcome};
