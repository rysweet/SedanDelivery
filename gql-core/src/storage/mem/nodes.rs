//! Node CRUD operations for [`InMemoryGraphStore`].
//!
//! Contains: create, get, delete, set/replace properties, add/remove labels.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::StorageError;
use crate::storage::{NodeData, NodeId};
use crate::types::Value;

use super::InMemoryGraphStore;

/// Create a node in the graph.
pub(crate) fn create_node_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    labels: Vec<String>,
    properties: HashMap<String, Value>,
) -> Result<NodeId, StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;

    // Honor pre-set ID from FanoutStore injection
    let raw_id = if let Some(Value::Int(i)) = properties.get("_sd2_id") {
        if *i < 0 {
            return Err(StorageError::Backend {
                msg: format!("_sd2_id must be non-negative, got {i}"),
            });
        }
        *i as u64
    } else {
        g.next_node_id(graph)?
    };
    let node_id = NodeId(raw_id);

    // Strip internal keys before storing
    let mut clean_props = properties;
    clean_props.remove("_sd2_id");
    clean_props.remove("_sd2_eid");

    // Check uniqueness constraints before inserting.
    g.check_uniqueness_constraints(&labels, &clean_props, None)?;

    let node = NodeData {
        id: node_id,
        labels: labels.clone(),
        properties: clean_props,
    };
    // Insert into BTreeMap property indexes before storing (need props ref).
    g.index_insert(raw_id, &labels, &node.properties);
    g.nodes.insert(raw_id, node);
    for label in &labels {
        g.label_index
            .entry(label.clone())
            .or_default()
            .insert(raw_id);
    }
    Ok(node_id)
}

/// Merge-update node properties (SET n += {...}).
pub(crate) fn set_node_properties_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    id: NodeId,
    properties: HashMap<String, Value>,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;

    // Check uniqueness constraints before mutating.
    {
        let node = g
            .nodes
            .get(&id.0)
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let mut merged = node.properties.clone();
        for (k, v) in &properties {
            merged.insert(k.clone(), v.clone());
        }
        g.check_uniqueness_constraints(&node.labels, &merged, Some(id.0))?;
    }

    let node = g
        .nodes
        .get_mut(&id.0)
        .ok_or(StorageError::NodeNotFound { id: id.0 })?;
    // Remove old index entries for changed keys, then insert new ones.
    let labels = node.labels.clone();
    let old_props: HashMap<String, Value> = properties
        .keys()
        .filter_map(|k| node.properties.get(k).map(|v| (k.clone(), v.clone())))
        .collect();
    g.index_remove(id.0, &labels, &old_props);
    // Merge: overwrite keys present in `properties`, leave others alone.
    let node = g
        .nodes
        .get_mut(&id.0)
        .ok_or(StorageError::NodeNotFound { id: id.0 })?;
    for (k, v) in &properties {
        node.properties.insert(k.clone(), v.clone());
    }
    g.index_insert(id.0, &labels, &properties);
    Ok(())
}

/// Full-replace node properties (SET n = {...}).
pub(crate) fn replace_node_properties_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    id: NodeId,
    properties: HashMap<String, Value>,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;

    // Check uniqueness constraints before mutating.
    let labels = {
        let node = g
            .nodes
            .get(&id.0)
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let labels = node.labels.clone();
        g.check_uniqueness_constraints(&labels, &properties, Some(id.0))?;
        labels
    };

    let node = g
        .nodes
        .get_mut(&id.0)
        .ok_or(StorageError::NodeNotFound { id: id.0 })?;
    let old_props = node.properties.clone();
    g.index_remove(id.0, &labels, &old_props);
    let node = g
        .nodes
        .get_mut(&id.0)
        .ok_or(StorageError::NodeNotFound { id: id.0 })?;
    node.properties = properties.clone();
    g.index_insert(id.0, &labels, &properties);
    Ok(())
}

/// Remove a single property from a node.
pub(crate) fn remove_node_property_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    id: NodeId,
    key: &str,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;
    let node = g
        .nodes
        .get_mut(&id.0)
        .ok_or(StorageError::NodeNotFound { id: id.0 })?;
    if let Some(old_val) = node.properties.remove(key) {
        let labels = node.labels.clone();
        let old_props = HashMap::from([(key.to_string(), old_val)]);
        g.index_remove(id.0, &labels, &old_props);
    }
    Ok(())
}

/// Add labels to a node.
pub(crate) fn add_node_labels_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    id: NodeId,
    labels: &[String],
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;
    let node = g
        .nodes
        .get_mut(&id.0)
        .ok_or(StorageError::NodeNotFound { id: id.0 })?;
    let new_labels: Vec<String> = labels
        .iter()
        .filter(|l| !node.labels.contains(l))
        .cloned()
        .collect();
    let props = node.properties.clone();
    for label in &new_labels {
        node.labels.push(label.clone());
    }
    // Update label index.
    for label in &new_labels {
        g.label_index.entry(label.clone()).or_default().insert(id.0);
    }
    // Index new label+property combinations.
    g.index_insert(id.0, &new_labels, &props);
    Ok(())
}

/// Remove labels from a node.
pub(crate) fn remove_node_labels_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    id: NodeId,
    labels: &[String],
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;
    let node = g
        .nodes
        .get_mut(&id.0)
        .ok_or(StorageError::NodeNotFound { id: id.0 })?;
    let props = node.properties.clone();
    for label in labels {
        node.labels.retain(|l| l != label);
    }
    // Remove from property indexes for removed labels.
    g.index_remove(id.0, labels, &props);
    // Update label index.
    for label in labels {
        if let Some(set) = g.label_index.get_mut(label) {
            set.remove(&id.0);
            if set.is_empty() {
                g.label_index.remove(label);
            }
        }
    }
    Ok(())
}

/// Delete a node, optionally detaching edges first.
pub(crate) fn delete_node_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    id: NodeId,
    detach: bool,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;

    if !g.nodes.contains_key(&id.0) {
        return Err(StorageError::NodeNotFound { id: id.0 });
    }

    // Check for dangling edges.
    let has_out = g.out_adj.get(&id.0).is_some_and(|v| !v.is_empty());
    let has_in = g.in_adj.get(&id.0).is_some_and(|v| !v.is_empty());

    if (has_out || has_in) && !detach {
        return Err(StorageError::Backend {
            msg: format!(
                "node #{} has relationships; use DETACH DELETE to delete it",
                id.0
            ),
        });
    }

    if detach {
        // Collect all edge IDs attached to this node (both directions).
        // Deduplicate to handle self-loops: a self-loop a->a appears in both
        // out_adj and in_adj; without dedup, g.edges.remove(eid) is called
        // twice and the second call skips reltype cleanup (returns None silently).
        let mut all_edge_ids: std::collections::HashSet<u64> = std::collections::HashSet::new();
        if let Some(adj) = g.out_adj.get(&id.0) {
            for e in adj {
                all_edge_ids.insert(e.edge_id.0);
            }
        }
        if let Some(adj) = g.in_adj.get(&id.0) {
            for e in adj {
                all_edge_ids.insert(e.edge_id.0);
            }
        }

        // Remove each incident edge exactly once.
        for eid in &all_edge_ids {
            if let Some(edge) = g.edges.remove(eid) {
                // Clean reltype index.
                if let Some(set) = g.reltype_index.get_mut(&edge.rel_type) {
                    set.remove(eid);
                    if set.is_empty() {
                        g.reltype_index.remove(&edge.rel_type);
                    }
                }
                // Clean the *other* endpoint's adjacency lists
                // (we will bulk-remove id's own lists after this loop).
                if edge.src.0 != id.0 {
                    if let Some(v) = g.out_adj.get_mut(&edge.src.0) {
                        v.retain(|e| e.edge_id.0 != *eid);
                    }
                }
                if edge.dst.0 != id.0 {
                    if let Some(v) = g.in_adj.get_mut(&edge.dst.0) {
                        v.retain(|e| e.edge_id.0 != *eid);
                    }
                }
            }
        }

        g.out_adj.remove(&id.0);
        g.in_adj.remove(&id.0);
    }

    // Remove from label index and property indexes.
    if let Some(node) = g.nodes.remove(&id.0) {
        g.index_remove(id.0, &node.labels, &node.properties);
        for label in &node.labels {
            if let Some(set) = g.label_index.get_mut(label) {
                set.remove(&id.0);
                if set.is_empty() {
                    g.label_index.remove(label);
                }
            }
        }
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::storage::mem::test_helpers::make_store;
    use crate::storage::{GraphStore, NodeId};
    use crate::types::Value;

    #[test]
    fn node_create_read_delete_roundtrip() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let id = store
            .create_node("test", vec!["Person".to_string()], props.clone())
            .unwrap();

        let node = store
            .get_node("test", id)
            .unwrap()
            .expect("node should exist");
        assert_eq!(node.id, id);
        assert_eq!(node.labels, vec!["Person".to_string()]);
        assert_eq!(node.properties["name"], Value::Str("Alice".into()));

        store.delete_node("test", id, false).unwrap();
        assert!(store.get_node("test", id).unwrap().is_none());
    }

    #[test]
    fn delete_node_without_detach_fails_if_has_edges() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        store
            .create_edge("test", a, b, "R".to_string(), HashMap::new())
            .unwrap();

        let result = store.delete_node("test", a, false);
        assert!(result.is_err(), "should refuse to delete connected node");
    }

    #[test]
    fn delete_node_with_detach_removes_edges() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let eid = store
            .create_edge("test", a, b, "R".to_string(), HashMap::new())
            .unwrap();

        store.delete_node("test", a, true).unwrap();
        assert!(store.get_node("test", a).unwrap().is_none());
        assert!(store.get_edge("test", eid).unwrap().is_none());
        assert!(store.in_edges("test", b).unwrap().is_empty());
    }

    #[test]
    fn detach_delete_cleans_both_directions() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let e1 = store
            .create_edge("test", a, b, "A_TO_B".to_string(), HashMap::new())
            .unwrap();
        let e2 = store
            .create_edge("test", b, a, "B_TO_A".to_string(), HashMap::new())
            .unwrap();

        store.delete_node("test", a, true).unwrap();

        assert!(store.get_edge("test", e1).unwrap().is_none());
        assert!(store.get_edge("test", e2).unwrap().is_none());
        assert!(store.out_edges("test", b).unwrap().is_empty());
        assert!(store.in_edges("test", b).unwrap().is_empty());
    }

    #[test]
    fn set_and_remove_node_properties() {
        let store = make_store();
        let id = store.create_node("test", vec![], HashMap::new()).unwrap();

        let mut props = HashMap::new();
        props.insert("x".to_string(), Value::Int(1));
        store.set_node_properties("test", id, props).unwrap();

        let node = store.get_node("test", id).unwrap().unwrap();
        assert_eq!(node.properties["x"], Value::Int(1));

        store.remove_node_property("test", id, "x").unwrap();
        let node = store.get_node("test", id).unwrap().unwrap();
        assert!(!node.properties.contains_key("x"));
    }

    #[test]
    fn add_and_remove_labels() {
        let store = make_store();
        let id = store
            .create_node("test", vec!["A".to_string()], HashMap::new())
            .unwrap();

        store
            .add_node_labels("test", id, &["B".to_string()])
            .unwrap();
        let node = store.get_node("test", id).unwrap().unwrap();
        assert!(node.labels.contains(&"B".to_string()));

        let b_nodes = store.find_nodes_by_label("test", "B").unwrap();
        assert!(b_nodes.contains(&id));

        store
            .remove_node_labels("test", id, &["A".to_string()])
            .unwrap();
        let node = store.get_node("test", id).unwrap().unwrap();
        assert!(!node.labels.contains(&"A".to_string()));
        let a_nodes = store.find_nodes_by_label("test", "A").unwrap();
        assert!(!a_nodes.contains(&id));
    }

    #[test]
    fn batch_fetch_nodes() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let fake = NodeId(9999);

        let results = store.get_nodes_batch("test", &[a, fake, b]).unwrap();
        assert_eq!(results.len(), 3);
        assert!(results[0].is_some());
        assert!(results[1].is_none());
        assert!(results[2].is_some());
    }

    #[test]
    fn all_node_ids_returns_all() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let mut ids = store.all_node_ids("test").unwrap();
        ids.sort();
        assert_eq!(ids, vec![a, b]);
    }
}
