//! In-memory graph store — primary implementation for unit tests, CLI, and Phase 0.
//!
//! All data lives in a single `RwLock<Inner>`. Reads acquire a shared lock; writes acquire
//! an exclusive lock. There are no sub-graph locks; the store is optimised for correctness
//! and test velocity, not raw throughput (the Redis backend handles production load).
//!
//! # Module layout
//!
//! The implementation is split across sub-modules for maintainability:
//! - [`indexable`] — `IndexableValue` Ord wrapper and `PropIndex` type aliases
//! - [`graph_inner`] — `GraphInner` per-graph state and index/constraint helpers
//! - [`store`] — `InMemoryGraphStore` struct, snapshot/merge, lock helpers
//! - [`nodes`] — Node CRUD (create, get, delete, properties, labels)
//! - [`edges`] — Edge CRUD (create, get, delete, properties)
//! - [`indexing`] — Property index management (create, drop, list, BTreeMap helpers)
//! - [`constraints`] — Uniqueness constraint create/drop/list helpers
//! - [`query`] — Read-only query helpers (range matching for fallback scans)
//! - [`graph_store_impl`] — `GraphStore` trait implementation (delegates to sub-modules)

mod constraints;
mod edges;
mod graph_inner;
mod graph_store_impl;
mod indexable;
mod indexing;
mod lifecycle;
mod nodes;
mod query;
mod store;

pub(crate) use graph_inner::GraphInner;
pub(crate) use indexable::{IndexableValue, PropIndex, PropIndexKey};
pub use store::InMemoryGraphStore;

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use {
    crate::error::StorageError,
    crate::storage::{
        AdjEntry, ConstraintDescriptor, ConstraintType, EdgeData, EdgeId, GraphStats, GraphStore,
        IndexDescriptor, NodeData, NodeId, RangeQuery,
    },
    crate::types::Value,
};

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::InMemoryGraphStore;
    use crate::storage::GraphStore;

    pub(crate) fn make_store() -> InMemoryGraphStore {
        let s = InMemoryGraphStore::new();
        s.create_graph("test").unwrap();
        s
    }

    pub(crate) fn make_indexed_store() -> InMemoryGraphStore {
        let s = make_store();
        s.create_index("test", "Person", "age").unwrap();
        s.create_index("test", "Person", "name").unwrap();
        s
    }
}

#[cfg(test)]
mod tests;
