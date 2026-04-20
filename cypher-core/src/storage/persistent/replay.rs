//! WAL/snapshot replay logic.
//!
//! Replay reads JSON-line entries and re-applies them to an
//! [`InMemoryGraphStore`] using the persisted IDs verbatim. Every operation
//! is made idempotent so that the same WAL stream can be replayed any number
//! of times into the same store without error and without changing the final
//! state.
//!
//! # Idempotency contract
//!
//! - `CreateNode` / `CreateEdge` use
//!   [`InMemoryGraphStore::create_node_with_id`] /
//!   [`InMemoryGraphStore::create_edge_with_id`]: the persisted ID is used
//!   directly (no assertion against an auto-allocated ID), and a pre-existing
//!   entity with that ID makes the operation a no-op.
//! - `DeleteNode` / `DeleteEdge` and property/label mutations on absent
//!   entities are treated as no-ops during replay.
//! - All remaining operations (`CreateGraph`, `CreateIndex`, …) are already
//!   idempotent via `*_exists` checks or the underlying store semantics.
//!
//! See issue #1701 and test `recovery_same_wal_replayed_n_times_is_stable`.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use super::wal::{inspect_wal_line, json_to_props, WalEntry, WalFrame};
use crate::error::StorageError;
use crate::storage::mem::InMemoryGraphStore;
use crate::storage::{EdgeId, GraphStore, NodeId};

/// Warn-once guard for legacy (unchecksummed v1) WAL/snapshot lines. Issue #1702:
/// new records are framed with `CRC:<hex8> <json>`; we still accept legacy
/// lines for on-disk compat but log a single warning per process.
static LEGACY_WARNED: AtomicBool = AtomicBool::new(false);

fn warn_legacy_once(path: &Path) {
    if !LEGACY_WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            path = %path.display(),
            "reading legacy (pre-#1702) unchecksummed WAL/snapshot records; \
             new records will be written with CRC32C framing"
        );
    }
}

/// Replay a file of JSON-line entries into an [`InMemoryGraphStore`].
///
/// Every entry is applied idempotently — see the module-level docs for the
/// full contract. Replaying the same file N times is guaranteed to converge
/// on a single state.
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

        let payload = match inspect_wal_line(line) {
            WalFrame::Checked(p) => p,
            WalFrame::Legacy(p) => {
                warn_legacy_once(path);
                p
            }
            WalFrame::Corrupt { expected, actual } => {
                return Err(StorageError::Serialization {
                    msg: format!(
                        "WAL CRC32C mismatch at line {}: expected {:08x}, computed {:08x} \
                         (fully-terminated record in {}) — on-disk corruption detected (#1702)",
                        line_num + 1,
                        expected,
                        actual,
                        path.display(),
                    ),
                });
            }
        };

        let entry: WalEntry =
            serde_json::from_str(payload).map_err(|e| StorageError::Serialization {
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
///
/// # Per-record checksums (issue #1702)
///
/// Each fully-committed line may be framed as `CRC:<hex8> <json>` where the
/// prefix encodes a CRC32C (Castagnoli) checksum of the JSON payload bytes.
/// If the prefix is present and the checksum does not match, the line is
/// real on-disk corruption (not a torn write, which by definition lacks the
/// trailing `\n`) and [`StorageError::Serialization`] is returned. Lines
/// without the prefix are treated as legacy (v1) records and replayed
/// unverified; a one-shot `warn!` is logged per process.
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
        let payload = match inspect_wal_line(line) {
            WalFrame::Checked(p) => p,
            WalFrame::Legacy(p) => {
                warn_legacy_once(path);
                p
            }
            WalFrame::Corrupt { expected, actual } => {
                return Err(StorageError::Serialization {
                    msg: format!(
                        "WAL CRC32C mismatch at line {}: expected {:08x}, computed {:08x} \
                         (fully-terminated record in {}) — on-disk corruption detected (#1702)",
                        line_num + 1,
                        expected,
                        actual,
                        path.display(),
                    ),
                });
            }
        };
        let entry: WalEntry =
            serde_json::from_str(payload).map_err(|e| StorageError::Serialization {
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
    /// Replay helper: treat `NodeNotFound` / `EdgeNotFound` as a no-op.
    ///
    /// Making non-create mutations tolerant to missing targets is what allows
    /// the same WAL to be replayed on top of itself (issue #1701): after one
    /// replay a deleted node stays deleted, and a second pass that re-issues
    /// the delete must not fail.
    fn skip_if_not_found(r: Result<(), StorageError>) -> Result<(), StorageError> {
        match r {
            Err(StorageError::NodeNotFound { .. } | StorageError::EdgeNotFound { .. }) => Ok(()),
            other => other,
        }
    }
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
            id,
        } => {
            let props = json_to_props(properties);
            // Use the persisted ID verbatim. Idempotent: a pre-existing node
            // with this ID makes this a no-op (see #1701).
            store.create_node_with_id(&graph, NodeId(id), labels, props)?;
        }
        WalEntry::CreateEdge {
            graph,
            src,
            dst,
            rel_type,
            properties,
            id,
        } => {
            let props = json_to_props(properties);
            store.create_edge_with_id(
                &graph,
                EdgeId(id),
                NodeId(src),
                NodeId(dst),
                rel_type,
                props,
            )?;
        }
        WalEntry::SetNodeProperties {
            graph,
            id,
            properties,
        } => {
            let props = json_to_props(properties);
            skip_if_not_found(store.set_node_properties(&graph, NodeId(id), props))?;
        }
        WalEntry::ReplaceNodeProperties {
            graph,
            id,
            properties,
        } => {
            let props = json_to_props(properties);
            skip_if_not_found(store.replace_node_properties(&graph, NodeId(id), props))?;
        }
        WalEntry::SetEdgeProperties {
            graph,
            id,
            properties,
        } => {
            let props = json_to_props(properties);
            skip_if_not_found(store.set_edge_properties(&graph, EdgeId(id), props))?;
        }
        WalEntry::ReplaceEdgeProperties {
            graph,
            id,
            properties,
        } => {
            let props = json_to_props(properties);
            skip_if_not_found(store.replace_edge_properties(&graph, EdgeId(id), props))?;
        }
        WalEntry::DeleteNode { graph, id } => {
            skip_if_not_found(store.delete_node(&graph, NodeId(id), true))?;
        }
        WalEntry::DeleteEdge { graph, id } => {
            skip_if_not_found(store.delete_edge(&graph, EdgeId(id)))?;
        }
        WalEntry::AddNodeLabels { graph, id, labels } => {
            skip_if_not_found(store.add_node_labels(&graph, NodeId(id), &labels))?;
        }
        WalEntry::RemoveNodeLabels { graph, id, labels } => {
            skip_if_not_found(store.remove_node_labels(&graph, NodeId(id), &labels))?;
        }
        WalEntry::RemoveNodeProperty { graph, id, key } => {
            skip_if_not_found(store.remove_node_property(&graph, NodeId(id), &key))?;
        }
        WalEntry::RemoveEdgeProperty { graph, id, key } => {
            skip_if_not_found(store.remove_edge_property(&graph, EdgeId(id), &key))?;
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
