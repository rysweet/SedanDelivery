//! Inline tests for the MERGE / MERGE RELATIONSHIP operators.

mod node;
mod rel;

use crate::storage::{GraphStore, InMemoryGraphStore};

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}
