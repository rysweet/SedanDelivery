//! Node property mutation: merge-set, full-replace, single-key remove.

use indexmap::IndexMap;
use std::sync::Arc;

use crate::error::StorageError;
use crate::storage::mem::InMemoryGraphStore;
use crate::storage::NodeId;
use crate::types::Value;

/// Merge-update node properties (SET n += {...}).
pub(crate) fn set_node_properties_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    id: NodeId,
    properties: IndexMap<String, Value>,
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
    let old_props: IndexMap<String, Value> = properties
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
    properties: IndexMap<String, Value>,
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
    if let Some(old_val) = node.properties.shift_remove(key) {
        let labels = node.labels.clone();
        let old_props = IndexMap::from([(key.to_string(), old_val)]);
        g.index_remove(id.0, &labels, &old_props);
    }
    Ok(())
}
