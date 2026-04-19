//! Property index management for [`InMemoryGraphStore`].
//!
//! Submodules:
//! - [`range_query`] — BTreeMap range query helpers (`Eq`, `Lt`, `Lte`, `Gt`,
//!   `Gte`, `Between`, `StartsWith`).
//! - [`lifecycle`] — `create_index`, `drop_index`, `list_indexes` implementations.

mod lifecycle;
mod range_query;

#[cfg(test)]
mod tests;

pub(crate) use lifecycle::{create_index_impl, drop_index_impl, list_indexes_impl};
pub(crate) use range_query::indexed_range_query;
