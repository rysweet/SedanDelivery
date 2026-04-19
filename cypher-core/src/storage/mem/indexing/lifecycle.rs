//! Index lifecycle: create, drop, list.

use std::sync::Arc;

use crate::error::StorageError;
use crate::storage::IndexDescriptor;

use super::super::{InMemoryGraphStore, IndexableValue};

/// List all indexes for a graph.
pub(crate) fn list_indexes_impl(
    store: &InMemoryGraphStore,
    graph: &str,
) -> Result<Vec<IndexDescriptor>, StorageError> {
    let snap = store.read_snapshot()?;
    let g = InMemoryGraphStore::require_graph(&snap, graph)?;
    Ok(g.indexes.clone())
}

/// Create a property index, backfilling existing nodes.
pub(crate) fn create_index_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    label: &str,
    property: &str,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;
    // Idempotent: only add if not already present.
    if g.has_index(label, property) {
        return Ok(());
    }
    let desc = IndexDescriptor {
        label: label.to_string(),
        property: property.to_string(),
    };
    g.indexes.push(desc);
    // Backfill: index all existing nodes that match this label+property.
    let node_ids: Vec<u64> = g
        .label_index
        .get(label)
        .map(|s| s.iter().copied().collect())
        .unwrap_or_default();
    for raw_id in node_ids {
        if let Some(node) = g.nodes.get(&raw_id) {
            if let Some(val) = node.properties.get(property) {
                if let Some(iv) = IndexableValue::from_value(val) {
                    g.prop_index
                        .entry((label.to_string(), property.to_string(), iv))
                        .or_default()
                        .insert(raw_id);
                }
            }
        }
    }
    Ok(())
}

/// Drop a property index and remove all BTreeMap entries for it.
pub(crate) fn drop_index_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    label: &str,
    property: &str,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;
    g.indexes
        .retain(|i| !(i.label == label && i.property == property));
    // Remove all BTreeMap entries for this label+property.
    let prefix = (label.to_string(), property.to_string());
    let keys_to_remove: Vec<super::super::PropIndexKey> = g
        .prop_index
        .range(
            (
                prefix.0.clone(),
                prefix.1.clone(),
                IndexableValue::Bool(false),
            )..,
        )
        .take_while(|((l, p, _), _)| l == &prefix.0 && p == &prefix.1)
        .map(|(k, _)| k.clone())
        .collect();
    for key in keys_to_remove {
        g.prop_index.remove(&key);
    }
    Ok(())
}
