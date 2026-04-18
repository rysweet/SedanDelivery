//! Pure-Rust parser, bytecode model, and compiler for Apache TinkerPop Gremlin traversals.
//!
//! This crate has no I/O and no runtime dependencies. It transforms Gremlin traversal
//! strings into [`cypher_core::planner::Operator`] trees that can be executed by any
//! cypher-core executor.
//!
//! # Pipeline
//!
//! ```text
//!   "g.V().count()"  --(text_parser)-->  [V, Count]  --(compiler)-->  Operator tree
//! ```
//!
//! # Quick start
//!
//! Parse a Gremlin string and compile it:
//!
//! ```
//! use gremlin_core::{parse_traversal, GremlinCompiler, GremlinStep};
//! use gremlin_core::types::GremlinValue;
//!
//! let no_params = |_: &str| -> Option<GremlinValue> { None };
//! let steps = parse_traversal("g.V().count()", &no_params).unwrap();
//! assert!(matches!(steps[0], GremlinStep::V(None)));
//!
//! let mut compiler = GremlinCompiler::new();
//! let operator = compiler.compile(&steps).unwrap();
//! ```
//!
//! # Modules
//!
//! - [`text_parser`] -- Recursive-descent parser: Gremlin string to `Vec<GremlinStep>`.
//! - [`bytecode`] -- The [`GremlinStep`] enum (~80 variants), [`Predicate`], and [`bytecode::Pop`].
//! - [`compiler`] -- [`GremlinCompiler`] transforms step sequences into `Operator` trees.
//! - [`types`] -- [`GremlinValue`] and [`Traverser`] with bidirectional `Value` conversions.
//! - [`graphson`] -- GraphSON v3 request/response message types with serde support.
//! - [`prelude`] -- Convenience re-exports: `use gremlin_core::prelude::*`.

pub mod bytecode;
pub mod compiler;
pub mod graphson;
pub mod prelude;
pub mod text_parser;
pub mod types;

pub use bytecode::{Bytecode, GremlinStep, Predicate};
pub use compiler::GremlinCompiler;
pub use graphson::{RequestMessage, ResponseMessage, ResponseResult, ResponseStatus};
pub use text_parser::{parse_traversal, TextParseError};
pub use types::{GremlinValue, Traverser};
