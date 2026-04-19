//! Shared test fixtures for scan-helper tests.

use crate::storage::{GraphStore, InMemoryGraphStore};

pub(super) fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}
