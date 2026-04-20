//! Graph-level lifecycle helpers: create/delete/exists/list/stats and
//! transaction snapshot/commit. Invoked by the `GraphStore` impl in `mod.rs`.

use std::sync::Arc;

use crate::error::StorageError;
use crate::storage::{validate_graph_name, GraphStats, GraphStore};

use super::graph_inner::GraphInner;
use super::store::InMemoryGraphStore;

pub(crate) fn create_graph_impl(
    store: &InMemoryGraphStore,
    name: &str,
) -> Result<(), StorageError> {
    validate_graph_name(name)?;
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    if map.contains_key(name) {
        return Err(StorageError::GraphAlreadyExists {
            name: name.to_string(),
        });
    }
    map.insert(name.to_string(), GraphInner::new());
    Ok(())
}

pub(crate) fn delete_graph_impl(
    store: &InMemoryGraphStore,
    name: &str,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    if map.remove(name).is_none() {
        return Err(StorageError::GraphNotFound {
            name: name.to_string(),
        });
    }
    Ok(())
}

pub(crate) fn graph_exists_impl(
    store: &InMemoryGraphStore,
    name: &str,
) -> Result<bool, StorageError> {
    let snap = store.read_snapshot()?;
    Ok(snap.contains_key(name))
}

pub(crate) fn list_graphs_impl(store: &InMemoryGraphStore) -> Result<Vec<String>, StorageError> {
    let snap = store.read_snapshot()?;
    let mut names: Vec<String> = snap.keys().cloned().collect();
    names.sort();
    Ok(names)
}

pub(crate) fn graph_stats_impl(
    store: &InMemoryGraphStore,
    graph: &str,
) -> Result<GraphStats, StorageError> {
    let snap = store.read_snapshot()?;
    let g = InMemoryGraphStore::require_graph(&snap, graph)?;
    Ok(GraphStats {
        node_count: g.nodes.len() as u64,
        edge_count: g.edges.len() as u64,
        label_count: g.label_index.len() as u32,
        rel_type_count: g.reltype_index.len() as u32,
    })
}

pub(crate) fn all_label_names_impl(
    store: &InMemoryGraphStore,
    graph: &str,
) -> Result<Vec<String>, StorageError> {
    let snap = store.read_snapshot()?;
    let g = InMemoryGraphStore::require_graph(&snap, graph)?;
    Ok(g.label_index.keys().cloned().collect())
}

pub(crate) fn tx_snapshot_impl(
    store: &InMemoryGraphStore,
    graphs: &[&str],
) -> Result<Option<Box<dyn GraphStore>>, StorageError> {
    let snap = store.snapshot(graphs)?;
    Ok(Some(Box::new(snap)))
}

pub(crate) fn tx_commit_impl(
    store: &InMemoryGraphStore,
    snapshot: &dyn GraphStore,
) -> Result<(), StorageError> {
    let snap = snapshot
        .as_any()
        .downcast_ref::<InMemoryGraphStore>()
        .ok_or_else(|| StorageError::backend("tx_commit: snapshot is not an InMemoryGraphStore"))?;
    store.merge_from(snap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> InMemoryGraphStore {
        InMemoryGraphStore::new()
    }

    #[test]
    fn create_then_exists() {
        let store = s();
        assert!(create_graph_impl(&store, "g1").is_ok());
        assert!(graph_exists_impl(&store, "g1").unwrap());
    }

    #[test]
    fn create_duplicate_errors() {
        let store = s();
        create_graph_impl(&store, "g1").unwrap();
        assert!(create_graph_impl(&store, "g1").is_err());
    }

    #[test]
    fn create_invalid_name_errors() {
        let store = s();
        assert!(create_graph_impl(&store, "").is_err());
    }

    #[test]
    fn delete_existing_graph() {
        let store = s();
        create_graph_impl(&store, "g1").unwrap();
        assert!(delete_graph_impl(&store, "g1").is_ok());
        assert!(!graph_exists_impl(&store, "g1").unwrap());
    }

    #[test]
    fn delete_missing_errors() {
        let store = s();
        assert!(delete_graph_impl(&store, "nope").is_err());
    }

    #[test]
    fn graph_exists_false_when_missing() {
        let store = s();
        assert!(!graph_exists_impl(&store, "nope").unwrap());
    }

    #[test]
    fn list_graphs_sorted() {
        let store = s();
        create_graph_impl(&store, "b").unwrap();
        create_graph_impl(&store, "a").unwrap();
        create_graph_impl(&store, "c").unwrap();
        let g = list_graphs_impl(&store).unwrap();
        assert_eq!(g, vec!["a".to_string(), "b".into(), "c".into()]);
    }

    #[test]
    fn graph_stats_empty_graph() {
        let store = s();
        create_graph_impl(&store, "g1").unwrap();
        let stats = graph_stats_impl(&store, "g1").unwrap();
        assert_eq!(stats.node_count, 0);
        assert_eq!(stats.edge_count, 0);
    }

    #[test]
    fn graph_stats_missing_errors() {
        let store = s();
        assert!(graph_stats_impl(&store, "nope").is_err());
    }

    #[test]
    fn all_label_names_empty() {
        let store = s();
        create_graph_impl(&store, "g1").unwrap();
        assert!(all_label_names_impl(&store, "g1").unwrap().is_empty());
    }

    #[test]
    fn all_label_names_missing_graph_errors() {
        let store = s();
        assert!(all_label_names_impl(&store, "nope").is_err());
    }

    #[test]
    fn tx_snapshot_returns_some() {
        let store = s();
        create_graph_impl(&store, "g1").unwrap();
        let snap = tx_snapshot_impl(&store, &["g1"]).unwrap();
        assert!(snap.is_some());
    }

    #[test]
    fn tx_commit_with_inmem_snapshot() {
        let store = s();
        create_graph_impl(&store, "g1").unwrap();
        let snap = tx_snapshot_impl(&store, &["g1"]).unwrap().unwrap();
        assert!(tx_commit_impl(&store, snap.as_ref()).is_ok());
    }
}
