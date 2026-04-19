//! WAL/snapshot replay logic.
//!
//! Replay reads JSON-line entries and re-applies them to a fresh
//! [`InMemoryGraphStore`]. ID-allocating operations verify that the
//! sequentially-allocated id matches the persisted id.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use super::wal::{json_to_props, WalEntry};
use crate::error::StorageError;
use crate::storage::mem::InMemoryGraphStore;
use crate::storage::{EdgeId, GraphStore, NodeId};

/// Replay a file of JSON-line WAL entries into an [`InMemoryGraphStore`].
///
/// For `CreateNode` and `CreateEdge` entries, we need to ensure the store's internal
/// ID sequence is advanced to match the persisted IDs. We do this by creating nodes/edges
/// via the store's normal API (which allocates IDs sequentially) and then skip if IDs
/// don't match — but this won't work because the store's sequence is opaque.
///
/// Instead we replay by directly creating nodes with the store's API. The IDs won't
/// match unless we seed the sequence. For snapshot replay this is fine because we replay
/// into an empty store whose sequence starts at 0 — as long as entries are in order,
/// the auto-assigned IDs will match the persisted IDs.
///
/// For robustness, we use a different approach: create the node/edge, then verify the
/// returned ID matches. If it doesn't, that's a bug in the snapshot ordering.
pub(super) fn replay_file(store: &InMemoryGraphStore, path: &Path) -> Result<(), StorageError> {
    let file = File::open(path).map_err(|e| StorageError::Backend {
        msg: format!("failed to open {}: {e}", path.display()),
    })?;
    let reader = BufReader::new(file);

    for (line_num, line_result) in reader.lines().enumerate() {
        let line = line_result.map_err(|e| StorageError::Backend {
            msg: format!("read error at line {}: {e}", line_num + 1),
        })?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry: WalEntry =
            serde_json::from_str(line).map_err(|e| StorageError::Serialization {
                msg: format!("WAL parse error at line {}: {e}", line_num + 1),
            })?;

        replay_entry(store, entry)?;
    }

    Ok(())
}

fn replay_entry(store: &InMemoryGraphStore, entry: WalEntry) -> Result<(), StorageError> {
    match entry {
        WalEntry::CreateGraph { name } => {
            // Idempotent: skip if already exists (snapshot + WAL overlap).
            if !store.graph_exists(&name)? {
                store.create_graph(&name)?;
            }
        }
        WalEntry::DeleteGraph { name } => {
            if store.graph_exists(&name)? {
                store.delete_graph(&name)?;
            }
        }
        WalEntry::CreateNode {
            graph,
            labels,
            properties,
            id: expected_id,
        } => {
            let props = json_to_props(properties);
            let actual_id = store.create_node(&graph, labels, props)?;
            if actual_id.0 != expected_id {
                return Err(StorageError::Backend {
                    msg: format!(
                        "replay ID mismatch: expected node {expected_id}, got {}",
                        actual_id.0
                    ),
                });
            }
        }
        WalEntry::CreateEdge {
            graph,
            src,
            dst,
            rel_type,
            properties,
            id: expected_id,
        } => {
            let props = json_to_props(properties);
            let actual_id = store.create_edge(&graph, NodeId(src), NodeId(dst), rel_type, props)?;
            if actual_id.0 != expected_id {
                return Err(StorageError::Backend {
                    msg: format!(
                        "replay ID mismatch: expected edge {expected_id}, got {}",
                        actual_id.0
                    ),
                });
            }
        }
        WalEntry::SetNodeProperties {
            graph,
            id,
            properties,
        } => {
            let props = json_to_props(properties);
            store.set_node_properties(&graph, NodeId(id), props)?;
        }
        WalEntry::ReplaceNodeProperties {
            graph,
            id,
            properties,
        } => {
            let props = json_to_props(properties);
            store.replace_node_properties(&graph, NodeId(id), props)?;
        }
        WalEntry::SetEdgeProperties {
            graph,
            id,
            properties,
        } => {
            let props = json_to_props(properties);
            store.set_edge_properties(&graph, EdgeId(id), props)?;
        }
        WalEntry::ReplaceEdgeProperties {
            graph,
            id,
            properties,
        } => {
            let props = json_to_props(properties);
            store.replace_edge_properties(&graph, EdgeId(id), props)?;
        }
        WalEntry::DeleteNode { graph, id } => {
            store.delete_node(&graph, NodeId(id), true)?;
        }
        WalEntry::DeleteEdge { graph, id } => {
            store.delete_edge(&graph, EdgeId(id))?;
        }
        WalEntry::AddNodeLabels { graph, id, labels } => {
            store.add_node_labels(&graph, NodeId(id), &labels)?;
        }
        WalEntry::RemoveNodeLabels { graph, id, labels } => {
            store.remove_node_labels(&graph, NodeId(id), &labels)?;
        }
        WalEntry::RemoveNodeProperty { graph, id, key } => {
            store.remove_node_property(&graph, NodeId(id), &key)?;
        }
        WalEntry::RemoveEdgeProperty { graph, id, key } => {
            store.remove_edge_property(&graph, EdgeId(id), &key)?;
        }
        WalEntry::CreateIndex {
            graph,
            label,
            property,
        } => {
            store.create_index(&graph, &label, &property)?;
        }
        WalEntry::DropIndex {
            graph,
            label,
            property,
        } => {
            store.drop_index(&graph, &label, &property)?;
        }
    }
    Ok(())
}
