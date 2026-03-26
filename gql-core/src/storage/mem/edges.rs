//! Edge CRUD operations for [`InMemoryGraphStore`].
//!
//! Contains: create, get, delete, set/replace properties.

use indexmap::IndexMap;
use std::sync::Arc;

use crate::error::StorageError;
use crate::storage::{AdjEntry, EdgeData, EdgeId, NodeId};
use crate::types::Value;

use super::InMemoryGraphStore;

/// Create an edge between two existing nodes.
pub(crate) fn create_edge_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    src: NodeId,
    dst: NodeId,
    rel_type: String,
    properties: IndexMap<String, Value>,
) -> Result<EdgeId, StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;
    // Validate that both endpoints exist.
    if !g.nodes.contains_key(&src.0) {
        return Err(StorageError::NodeNotFound { id: src.0 });
    }
    if !g.nodes.contains_key(&dst.0) {
        return Err(StorageError::NodeNotFound { id: dst.0 });
    }

    // Honor pre-set ID from FanoutStore injection
    let raw_id = if let Some(Value::Int(i)) = properties.get("_sd2_eid") {
        if *i < 0 {
            return Err(StorageError::Backend {
                msg: format!("_sd2_eid must be non-negative, got {i}"),
            });
        }
        *i as u64
    } else {
        g.next_edge_id(graph)?
    };
    let edge_id = EdgeId(raw_id);

    // Strip internal keys before storing
    let mut clean_props = properties;
    clean_props.shift_remove("_sd2_id");
    clean_props.shift_remove("_sd2_eid");

    let edge = EdgeData {
        id: edge_id,
        rel_type: rel_type.clone(),
        src,
        dst,
        properties: clean_props,
    };
    g.edges.insert(raw_id, edge);

    // Update adjacency lists.
    g.out_adj.entry(src.0).or_default().push(AdjEntry {
        edge_id,
        neighbor_id: dst,
        rel_type: rel_type.clone(),
    });
    g.in_adj.entry(dst.0).or_default().push(AdjEntry {
        edge_id,
        neighbor_id: src,
        rel_type: rel_type.clone(),
    });

    // Update rel-type index.
    g.reltype_index.entry(rel_type).or_default().insert(raw_id);
    Ok(edge_id)
}

/// Merge-update edge properties (SET r += {...}).
pub(crate) fn set_edge_properties_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    id: EdgeId,
    properties: IndexMap<String, Value>,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;
    let edge = g
        .edges
        .get_mut(&id.0)
        .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
    for (k, v) in properties {
        edge.properties.insert(k, v);
    }
    Ok(())
}

/// Full-replace edge properties (SET r = {...}).
pub(crate) fn replace_edge_properties_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    id: EdgeId,
    properties: IndexMap<String, Value>,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;
    let edge = g
        .edges
        .get_mut(&id.0)
        .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
    edge.properties = properties;
    Ok(())
}

/// Remove a single property from an edge.
pub(crate) fn remove_edge_property_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    id: EdgeId,
    key: &str,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;
    let edge = g
        .edges
        .get_mut(&id.0)
        .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
    edge.properties.shift_remove(key);
    Ok(())
}

/// Delete an edge and clean up adjacency/reltype indexes.
pub(crate) fn delete_edge_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    id: EdgeId,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;

    let edge = g
        .edges
        .remove(&id.0)
        .ok_or(StorageError::EdgeNotFound { id: id.0 })?;

    // Remove from adjacency lists.
    if let Some(v) = g.out_adj.get_mut(&edge.src.0) {
        v.retain(|e| e.edge_id != id);
    }
    if let Some(v) = g.in_adj.get_mut(&edge.dst.0) {
        v.retain(|e| e.edge_id != id);
    }

    // Remove from rel-type index.
    if let Some(set) = g.reltype_index.get_mut(&edge.rel_type) {
        set.remove(&id.0);
        if set.is_empty() {
            g.reltype_index.remove(&edge.rel_type);
        }
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use crate::storage::mem::test_helpers::make_store;
    use crate::storage::{EdgeId, GraphStore, NodeId};
    use crate::types::Value;

    #[test]
    fn edge_create_read_delete_roundtrip() {
        let store = make_store();
        let a = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let b = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let mut props = IndexMap::new();
        props.insert("weight".to_string(), Value::Float(1.5));
        let eid = store
            .create_edge("test", a, b, "KNOWS".to_string(), props)
            .unwrap();

        let edge = store
            .get_edge("test", eid)
            .unwrap()
            .expect("edge should exist");
        assert_eq!(edge.src, a);
        assert_eq!(edge.dst, b);
        assert_eq!(edge.rel_type, "KNOWS");
        assert_eq!(edge.properties["weight"], Value::Float(1.5));

        store.delete_edge("test", eid).unwrap();
        assert!(store.get_edge("test", eid).unwrap().is_none());
    }

    #[test]
    fn create_edge_requires_existing_nodes() {
        let store = make_store();
        let a = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let bad = NodeId(9999);
        let result = store.create_edge("test", a, bad, "R".to_string(), IndexMap::new());
        assert!(matches!(
            result,
            Err(crate::error::StorageError::NodeNotFound { .. })
        ));
    }

    #[test]
    fn batch_fetch_edges() {
        let store = make_store();
        let a = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let b = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let eid = store
            .create_edge("test", a, b, "R".to_string(), IndexMap::new())
            .unwrap();
        let fake = EdgeId(9999);

        let results = store.get_edges_batch("test", &[eid, fake]).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].is_some());
        assert!(results[1].is_none());
    }
}
