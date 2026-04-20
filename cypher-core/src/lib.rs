//! Pure Rust openCypher/GQL query engine — no Redis dependency.
//!
//! `cypher-core` provides a complete pipeline for executing openCypher queries:
//! parsing, semantic validation, physical plan construction, and tree-walking
//! execution against a pluggable [`storage::GraphStore`] backend.
//!
//! # Pipeline
//!
//! ```text
//! query &str ─→ parser::parse_query ─→ ast::Query
//!     ─→ planner::QueryPlanner::plan ─→ planner::Operator
//!     ─→ executor::QueryExecutor::execute ─→ executor::QueryResult
//! ```
//!
//! # Quick example
//!
//! ```
//! use cypher_core::parser::parse_query;
//! use cypher_core::planner::QueryPlanner;
//! use cypher_core::executor::{QueryExecutor, ExecutionBudget};
//! use cypher_core::storage::{GraphStore, InMemoryGraphStore};
//!
//! let store = InMemoryGraphStore::default();
//! store.create_graph("g").unwrap();
//!
//! let ast = parse_query("RETURN 1 + 2 AS result").unwrap();
//! let plan = QueryPlanner::new(&store, "g").plan(&ast).unwrap();
//! let mut budget = ExecutionBudget::default_budget();
//! let result = QueryExecutor::new(&store).execute("g", &plan, &mut budget).unwrap();
//!
//! assert_eq!(result.rows[0]["result"], cypher_core::types::Value::Int(3));
//! ```
//!
//! # Crate consumers
//!
//! - [`sedan-cypher-embedded`] — single-call API wrapping this crate
//! - `sd2-redis-store` / `sd2-redis-module` — Redis/Valkey integration
//! - `gql-cli` — command-line interface

#[cfg(test)]
extern crate self as cypher_core;

#[cfg(test)]
mod coverage_smoke_tests;

#[cfg(test)]
mod coverage_targeted_tests;

#[cfg(test)]
mod coverage_targeted_tests2;

#[cfg(test)]
mod coverage_targeted_tests3;

#[cfg(test)]
mod coverage_targeted_tests4;

#[cfg(test)]
mod coverage_targeted_tests5;

#[cfg(test)]
mod coverage_targeted_tests6;

#[cfg(test)]
mod coverage_targeted_tests7;

#[cfg(test)]
mod coverage_targeted_tests8;

#[cfg(test)]
mod coverage_targeted_tests9;

#[cfg(test)]
mod coverage_targeted_tests10;

#[cfg(test)]
mod coverage_targeted_tests11;

#[cfg(test)]
mod coverage_targeted_tests12;

pub mod ast;
pub mod changelog;
pub mod config;
pub mod error;
pub mod executor;
pub mod parser;
pub mod plan_cache;
pub mod planner;
pub mod procedure;
pub mod serde_value;
pub mod storage;
pub mod types;

/// Common re-exports for typical usage.
///
/// ```
/// use cypher_core::prelude::*;
///
/// let store = InMemoryGraphStore::default();
/// store.create_graph("g").unwrap();
///
/// let ast = parse_query("RETURN 1 + 2 AS result").unwrap();
/// let plan = QueryPlanner::new(&store, "g").plan(&ast).unwrap();
/// let mut budget = ExecutionBudget::default_budget();
/// let result = QueryExecutor::new(&store).execute("g", &plan, &mut budget).unwrap();
///
/// assert_eq!(result.rows[0]["result"], Value::Int(3));
/// ```
pub mod prelude {
    pub use crate::error::GqlError;
    pub use crate::executor::{ExecutionBudget, QueryExecutor, QueryResult};
    pub use crate::parser::parse_query;
    pub use crate::planner::QueryPlanner;
    pub use crate::storage::{GraphStore, InMemoryGraphStore};
    pub use crate::types::Value;
}
