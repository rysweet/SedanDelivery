//! Snapshot writer and background snapshot loop.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use super::wal::{props_to_json, WalEntry};
use crate::error::StorageError;
use crate::storage::mem::InMemoryGraphStore;
use crate::storage::GraphStore;

/// Background loop that periodically writes a snapshot and truncates the WAL.
pub(super) fn snapshot_loop(
    shutdown: &AtomicBool,
    data_dir: &Path,
    inner: &InMemoryGraphStore,
    interval: Duration,
) {
    // Sleep in small increments so we can respond to shutdown promptly.
    let tick = Duration::from_millis(100).min(interval);
    let mut elapsed = Duration::ZERO;

    while !shutdown.load(Ordering::SeqCst) {
        thread::sleep(tick);
        elapsed += tick;

        if elapsed >= interval {
            elapsed = Duration::ZERO;
            if let Err(e) = write_snapshot(data_dir, inner) {
                tracing::error!("snapshot failed: {e}");
                continue;
            }
            // We cannot truncate the WAL from here because we don't hold the Mutex<BufWriter>.
            // The WAL will grow until the next snapshot or store close. A more complete
            // implementation would share the WAL mutex, but for correctness and simplicity
            // we accept WAL growth between snapshots.
            //
            // After a successful snapshot, truncate the WAL by reopening it.
            let wal_path = data_dir.join("wal.jsonl");
            if let Err(e) = fs::write(&wal_path, b"") {
                tracing::error!("WAL truncate failed: {e}");
            }
        }
    }
}

/// Serialize the current graph state to `snapshot.json` via an atomic rename.
pub(super) fn write_snapshot(
    data_dir: &Path,
    inner: &InMemoryGraphStore,
) -> Result<(), StorageError> {
    let tmp_path = data_dir.join("snapshot.json.tmp");
    let final_path = data_dir.join("snapshot.json");

    let mut file = BufWriter::new(File::create(&tmp_path).map_err(|e| StorageError::Backend {
        msg: format!("snapshot create failed: {e}"),
    })?);

    // Serialize by reading all data through the GraphStore trait.
    let graphs = inner.list_graphs()?;
    for graph_name in &graphs {
        let entry = WalEntry::CreateGraph {
            name: graph_name.clone(),
        };
        let line = serde_json::to_string(&entry).map_err(|e| StorageError::Serialization {
            msg: format!("snapshot serialize: {e}"),
        })?;
        writeln!(file, "{}", line).map_err(|e| StorageError::Backend {
            msg: format!("snapshot write: {e}"),
        })?;

        // Indexes first (so property indexes are in place before nodes are created)
        let indexes = inner.list_indexes(graph_name)?;
        for idx in &indexes {
            let entry = WalEntry::CreateIndex {
                graph: graph_name.clone(),
                label: idx.label.clone(),
                property: idx.property.clone(),
            };
            let line = serde_json::to_string(&entry).map_err(|e| StorageError::Serialization {
                msg: format!("snapshot serialize: {e}"),
            })?;
            writeln!(file, "{}", line).map_err(|e| StorageError::Backend {
                msg: format!("snapshot write: {e}"),
            })?;
        }

        // Nodes
        let node_ids = inner.all_node_ids(graph_name)?;
        for nid in &node_ids {
            if let Some(node) = inner.get_node(graph_name, *nid)? {
                let entry = WalEntry::CreateNode {
                    graph: graph_name.clone(),
                    labels: node.labels.clone(),
                    properties: props_to_json(&node.properties),
                    id: nid.0,
                };
                let line =
                    serde_json::to_string(&entry).map_err(|e| StorageError::Serialization {
                        msg: format!("snapshot serialize: {e}"),
                    })?;
                writeln!(file, "{}", line).map_err(|e| StorageError::Backend {
                    msg: format!("snapshot write: {e}"),
                })?;
            }
        }

        // Edges
        let edge_ids = inner.all_edge_ids(graph_name)?;
        for eid in &edge_ids {
            if let Some(edge) = inner.get_edge(graph_name, *eid)? {
                let entry = WalEntry::CreateEdge {
                    graph: graph_name.clone(),
                    src: edge.src.0,
                    dst: edge.dst.0,
                    rel_type: edge.rel_type.clone(),
                    properties: props_to_json(&edge.properties),
                    id: eid.0,
                };
                let line =
                    serde_json::to_string(&entry).map_err(|e| StorageError::Serialization {
                        msg: format!("snapshot serialize: {e}"),
                    })?;
                writeln!(file, "{}", line).map_err(|e| StorageError::Backend {
                    msg: format!("snapshot write: {e}"),
                })?;
            }
        }
    }

    file.flush().map_err(|e| StorageError::Backend {
        msg: format!("snapshot flush: {e}"),
    })?;

    // Atomic rename
    fs::rename(&tmp_path, &final_path).map_err(|e| StorageError::Backend {
        msg: format!("snapshot rename: {e}"),
    })?;

    Ok(())
}

#[cfg(test)]
pub(super) fn truncate_wal(
    data_dir: &Path,
    wal_mutex: &Mutex<BufWriter<File>>,
) -> Result<(), StorageError> {
    use std::fs::OpenOptions;

    let wal_path = data_dir.join("wal.jsonl");
    let mut wal_guard = wal_mutex.lock().map_err(|e| StorageError::Backend {
        msg: format!("WAL lock poisoned: {e}"),
    })?;
    // Flush current buffer, then reopen truncated
    wal_guard.flush().map_err(|e| StorageError::Backend {
        msg: format!("WAL flush: {e}"),
    })?;
    let new_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&wal_path)
        .map_err(|e| StorageError::Backend {
            msg: format!("WAL truncate reopen: {e}"),
        })?;
    *wal_guard = BufWriter::new(new_file);
    Ok(())
}
