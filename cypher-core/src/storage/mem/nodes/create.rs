//! Node creation.

use indexmap::IndexMap;
use std::sync::Arc;

use crate::error::StorageError;
use crate::storage::mem::InMemoryGraphStore;
use crate::storage::{NodeData, NodeId};
use crate::types::Value;

/// Create a node in the graph.
pub(crate) fn create_node_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    labels: Vec<String>,
    properties: IndexMap<String, Value>,
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

    // Bug #1204: reject duplicate _sd2_id before any index/storage mutation.
    if g.nodes.contains_key(&raw_id) {
        return Err(StorageError::Backend {
            msg: format!("node with id {raw_id} already exists"),
        });
    }

    // Strip internal keys before storing
    let mut clean_props = properties;
    clean_props.shift_remove("_sd2_id");
    clean_props.shift_remove("_sd2_eid");

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

/// Create a node with an explicit, caller-supplied [`NodeId`].
///
/// Unlike [`create_node_impl`], this bypasses the per-graph ID allocator and
/// instead uses `node_id` verbatim. The store's internal `edge_seq` counter
/// is advanced to `max(current, node_id)` so that any subsequently
/// auto-allocated IDs will not collide with the injected one.
///
/// Idempotent: if a node with `node_id` already exists in `graph`, this is a
/// no-op and returns `Ok(())`. This property is what makes WAL replay
/// in-place idempotent (issue #1701).
pub(crate) fn create_node_with_id_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    node_id: NodeId,
    labels: Vec<String>,
    properties: IndexMap<String, Value>,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;

    // Idempotent: if this ID is already present, nothing to do.
    if g.nodes.contains_key(&node_id.0) {
        return Ok(());
    }

    // Advance the shared seq counter so future auto-allocated IDs skip past
    // this one. `next_node_id` uses `edge_seq` as the shared source of truth.
    if node_id.0 > g.edge_seq {
        g.edge_seq = node_id.0;
        g.node_seq = node_id.0;
    }

    // Strip internal keys just like the normal create path.
    let mut clean_props = properties;
    clean_props.shift_remove("_sd2_id");
    clean_props.shift_remove("_sd2_eid");

    g.check_uniqueness_constraints(&labels, &clean_props, None)?;

    let node = NodeData {
        id: node_id,
        labels: labels.clone(),
        properties: clean_props,
    };
    g.index_insert(node_id.0, &labels, &node.properties);
    g.nodes.insert(node_id.0, node);
    for label in &labels {
        g.label_index
            .entry(label.clone())
            .or_default()
            .insert(node_id.0);
    }
    Ok(())
}
