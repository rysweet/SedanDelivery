//! WAL/snapshot replay logic.
//!
//! Replay reads JSON-line entries and re-applies them to a fresh
//! [`InMemoryGraphStore`]. ID-allocating operations verify that the
//! sequentially-allocated id matches the persisted id.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::Path;

use super::wal::{json_to_props, WalEntry};
use crate::error::StorageError;
use crate::storage::mem::InMemoryGraphStore;
use crate::storage::{EdgeId, GraphStore, NodeId};

/// Replay a file of JSON-line entries into an [`InMemoryGraphStore`].
///
/// This is the **strict** replay path used for snapshot files: every line must
/// parse, and any parse error returns [`StorageError::Serialization`]. Snapshots
/// are written atomically (write-temp-then-rename), so a partial snapshot is a
/// real corruption, not a torn write.
///
/// For replaying the WAL file (which may have a torn trailing record from a
/// crash mid-`fsync`), use [`replay_wal_file`] instead.
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

/// Replay a WAL file, tolerating a single torn trailing record.
///
/// # Durability contract
///
/// The WAL writer always emits records via `writeln!`, so every fully-committed
/// record is terminated by `\n`. If the file's last byte is **not** `\n`, the
/// trailing bytes are an interrupted write (e.g., process killed mid-`fsync`):
///
/// * All bytes up to and including the **last** `\n` are fully-committed records
///   and are replayed strictly — any parse error here is real corruption and
///   returns [`StorageError::Serialization`].
/// * The unterminated trailing bytes are discarded, the file is truncated to the
///   end of the last complete record, and a `tracing::warn!` is logged. This
///   restores the on-disk file to a clean record boundary so subsequent appends
///   start aligned.
///
/// If the file ends with `\n`, every record is treated as committed and parse
/// failures propagate as errors (no torn-tail recovery is attempted — the data
/// is intact on disk and a parse error means real corruption, not a torn write).
pub(super) fn replay_wal_file(store: &InMemoryGraphStore, path: &Path) -> Result<(), StorageError> {
    let bytes = fs::read(path).map_err(|e| StorageError::Backend {
        msg: format!("failed to read {}: {e}", path.display()),
    })?;
    if bytes.is_empty() {
        return Ok(());
    }

    // Split into the fully-committed prefix (ends at the last newline) and any
    // trailing bytes that are NOT newline-terminated.
    let (committed, trailing): (&[u8], &[u8]) = match bytes.iter().rposition(|&b| b == b'\n') {
        Some(idx) => (&bytes[..=idx], &bytes[idx + 1..]),
        None => (&[], &bytes[..]),
    };

    // Strict replay of every fully-committed line. Any parse error here is
    // genuine corruption, not a torn write.
    for (line_num, line) in committed.split(|&b| b == b'\n').enumerate() {
        let line = std::str::from_utf8(line)
            .map_err(|e| StorageError::Serialization {
                msg: format!("WAL utf8 error at line {}: {e}", line_num + 1),
            })?
            .trim();
        if line.is_empty() {
            continue;
        }
        let entry: WalEntry =
            serde_json::from_str(line).map_err(|e| StorageError::Serialization {
                msg: format!("WAL parse error at line {}: {e}", line_num + 1),
            })?;
        replay_entry(store, entry)?;
    }

    // Trailing unterminated bytes → torn write. Truncate to the last clean
    // record boundary so subsequent appends are aligned. We do NOT attempt to
    // parse-and-replay these bytes even if they happen to be valid JSON: the
    // writer's contract is that a record is committed iff it has a trailing
    // newline, so anything without one was never durable.
    if !trailing.is_empty() {
        let truncate_to = committed.len() as u64;
        tracing::warn!(
            path = %path.display(),
            torn_bytes = trailing.len(),
            truncate_to,
            "WAL torn tail detected (unterminated trailing bytes); truncating to last record boundary"
        );
        let f = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| StorageError::Backend {
                msg: format!("failed to open WAL for truncation: {e}"),
            })?;
        f.set_len(truncate_to).map_err(|e| StorageError::Backend {
            msg: format!("failed to truncate WAL torn tail: {e}"),
        })?;
        f.sync_all().map_err(|e| StorageError::Backend {
            msg: format!("failed to sync WAL after torn-tail truncation: {e}"),
        })?;
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
