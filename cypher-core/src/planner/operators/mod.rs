//! Physical plan operator types and supporting AST-like structures.
//!
//! Defines the complete physical plan operator set (33 operators) plus
//! supporting types (CREATE patterns, SET/REMOVE items, aggregation functions).
//!
//! The module is split into focused submodules:
//!
//! - [`operator`] — the [`Operator`] enum (single indivisible declaration
//!   holding all 33 physical plan variants).
//! - [`aggregation`] — [`AggFunction`] enum for aggregation kinds
//!   (Count/Sum/Avg/Min/Max/Collect/StDev/Percentile/...).
//! - [`mutation`] — CREATE/SET/REMOVE supporting types
//!   ([`CreatePattern`], [`CreateNodeSpec`], [`CreateEdgeSpec`],
//!   [`SetItem`], [`RemoveItem`]).
//! - [`gremlin_types`] — Gremlin-specific helper enums
//!   ([`GremlinPop`], [`GremlinSelectTraversal`]).

pub mod aggregation;
pub mod gremlin_types;
pub mod mutation;
pub mod operator;

pub use aggregation::AggFunction;
pub use gremlin_types::{GremlinPop, GremlinSelectTraversal};
pub use mutation::{CreateEdgeSpec, CreateNodeSpec, CreatePattern, RemoveItem, SetItem};
pub use operator::Operator;
