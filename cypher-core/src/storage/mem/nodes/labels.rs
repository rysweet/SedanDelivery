//! Add/remove labels on existing nodes.

use std::sync::Arc;

use crate::error::StorageError;
use crate::storage::mem::InMemoryGraphStore;
use crate::storage::NodeId;

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
