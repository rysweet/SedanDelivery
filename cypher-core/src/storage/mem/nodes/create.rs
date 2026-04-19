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
