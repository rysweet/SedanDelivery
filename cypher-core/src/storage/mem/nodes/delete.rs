//! Node deletion (with optional `DETACH` semantics).

use std::sync::Arc;

use crate::error::StorageError;
use crate::storage::mem::InMemoryGraphStore;
use crate::storage::NodeId;

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
