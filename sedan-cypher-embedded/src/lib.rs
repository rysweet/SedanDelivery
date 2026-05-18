//! Embeddable openCypher query engine.
//!
//! `sedan-cypher-embedded` is a thin lens over [`cypher_core`]: the same
//! parser, planner, and executor that power the SedanDelivery network
//! servers, exposed as a single in-process [`CypherEngine`] type. There is
//! no network, no authentication, and no background thread — every call
//! runs on the caller's thread against a [`GraphStore`] you supply.
//!
//! # When to use
//!
//! Reach for this crate when you want:
//!
//! - openCypher inside a Rust application without operating a separate
//!   database process,
//! - deterministic in-process tests against an [`InMemoryGraphStore`],
//! - a building block for a custom storage backend (implement
//!   [`GraphStore`] and pass it to [`CypherEngine::execute`]).
//!
//! For the full networked database, use the `sd2-api`, `sedandelivery-bolt`,
//! or `sd2-redis-module` crates instead.
//!
//! # Installation
//!
//! ```text
//! cargo add sedan-cypher-embedded
//! ```
//!
//! Or, in `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! sedan-cypher-embedded = "*"
//! ```
//!
//! # Quick start
//!
//! ```
//! use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};
//!
//! let engine = CypherEngine::new();
//! let store = InMemoryGraphStore::default();
//! store.create_graph("g").unwrap();
//!
//! engine.execute(
//!     &store, "g",
//!     "CREATE (:Person {name: 'Alice', age: 30}), (:Person {name: 'Bob', age: 25})",
//! ).unwrap();
//!
//! let result = engine.execute(
//!     &store, "g",
//!     "MATCH (p:Person) RETURN count(p) AS n, avg(p.age) AS avg_age",
//! ).unwrap();
//! assert_eq!(result.rows[0]["n"], Value::Int(2));
//! ```
//!
//! # Parameters and procedures
//!
//! Use [`CypherEngine::execute_with_parameters`] when the query contains
//! `$name` placeholders or invokes user-defined procedures via `CALL`. See
//! that method's documentation for a complete example.
//!
//! # Error handling
//!
//! All execution paths return [`GqlError`]. The variant identifies the
//! phase that failed (parse / plan / execution / storage / type):
//!
//! ```
//! use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, GqlError};
//!
//! let engine = CypherEngine::new();
//! let store = InMemoryGraphStore::default();
//! store.create_graph("g").unwrap();
//!
//! match engine.execute(&store, "g", "NOT VALID CYPHER") {
//!     Err(GqlError::Parse(_)) => { /* syntax error */ }
//!     Err(_) => { /* plan, execution, storage, or type error */ }
//!     Ok(_) => unreachable!(),
//! }
//! ```
//!
//! # Plan caching
//!
//! [`CypherEngine::execute`] maintains an internal plan cache so repeated
//! query strings skip parsing and planning. Cached plans always run against
//! the live store, so writes (CREATE / SET / DELETE / MERGE / REMOVE)
//! observe fresh data. Call [`CypherEngine::clear_cache`] after schema-
//! changing DDL such as `CREATE INDEX`.

#![warn(missing_docs)]

mod engine;

pub use cypher_core::error::GqlError;
pub use cypher_core::executor::{ExecutionBudget, MutationStats, QueryResult, Row};
pub use cypher_core::procedure::ProcedureRegistry;
pub use cypher_core::storage::{GraphStore, InMemoryGraphStore};
pub use cypher_core::types::Value;

pub use engine::CypherEngine;
