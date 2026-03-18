//! Gremlin bytecode types, GraphSON serialization, and compiler to gql-core Operator trees.
//!
//! This crate is pure Rust (no I/O). It provides:
//! - [`GremlinValue`] — the Gremlin value type with conversions to/from [`gql_core::types::Value`]
//! - [`GremlinStep`] — typed representations of ~50 Gremlin traversal steps
//! - [`GremlinCompiler`] — transforms a step list into a [`gql_core::planner::Operator`] tree
//! - GraphSON v3 request/response message types

pub mod bytecode;
pub mod compiler;
pub mod graphson;
pub mod steps;
pub mod text_parser;
pub mod types;

pub use bytecode::{Bytecode, GremlinStep, Predicate};
pub use compiler::GremlinCompiler;
pub use graphson::{RequestMessage, ResponseMessage, ResponseResult, ResponseStatus};
pub use text_parser::{parse_traversal, TextParseError};
pub use types::{GremlinValue, Traverser};
