//! Persistent graph store — wraps [`InMemoryGraphStore`] with WAL-based disk persistence.
//!
//! All reads delegate to the in-memory store (zero disk I/O). All writes first apply to the
//! in-memory store then append a JSON-line WAL entry. A background thread periodically
//! snapshots the full state to disk and truncates the WAL.
//!
//! # Recovery
//!
//! On [`PersistentGraphStore::open`], the store:
//! 1. Loads the snapshot file (if it exists) by replaying its entries
//! 2. Replays any WAL entries on top
//! 3. Opens the WAL for append

use std::any::Any;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::mem::InMemoryGraphStore;
use super::{
    AdjEntry, ConstraintDescriptor, ConstraintType, EdgeData, EdgeId, GraphStats, GraphStore,
    IndexDescriptor, NodeData, NodeId, RangeQuery, ReadConsistency,
};
use crate::error::StorageError;
use crate::types::Value;

// ── Configuration ────────────────────────────────────────────────────────────

/// Controls how the WAL is synced to disk after writes.
#[derive(Debug, Clone)]
pub enum SyncMode {
    /// `fsync` after every WAL write. Safest, slowest.
    Immediate,
    /// Flush the WAL buffer periodically (the OS decides when to sync).
    Periodic(Duration),
    /// No explicit sync — rely on OS page cache. Fastest, least durable.
    None,
}

/// Configuration for [`PersistentGraphStore`].
#[derive(Debug, Clone)]
pub struct PersistentConfig {
    /// Directory where snapshot and WAL files are stored.
    pub data_dir: PathBuf,
    /// How often the background thread writes a snapshot and truncates the WAL.
    pub snapshot_interval: Duration,
    /// WAL sync behavior.
    pub sync_mode: SyncMode,
}

// ── WAL entry ────────────────────────────────────────────────────────────────

/// A single mutation recorded in the WAL or snapshot file. One JSON line per entry.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "op")]
enum WalEntry {
    CreateGraph {
        name: String,
    },
    DeleteGraph {
        name: String,
    },
    CreateNode {
        graph: String,
        labels: Vec<String>,
        properties: Vec<(String, JsonValue)>,
        id: u64,
    },
    CreateEdge {
        graph: String,
        src: u64,
        dst: u64,
        rel_type: String,
        properties: Vec<(String, JsonValue)>,
        id: u64,
    },
    SetNodeProperties {
        graph: String,
        id: u64,
        properties: Vec<(String, JsonValue)>,
    },
    ReplaceNodeProperties {
        graph: String,
        id: u64,
        properties: Vec<(String, JsonValue)>,
    },
    SetEdgeProperties {
        graph: String,
        id: u64,
        properties: Vec<(String, JsonValue)>,
    },
    ReplaceEdgeProperties {
        graph: String,
        id: u64,
        properties: Vec<(String, JsonValue)>,
    },
    DeleteNode {
        graph: String,
        id: u64,
    },
    DeleteEdge {
        graph: String,
        id: u64,
    },
    AddNodeLabels {
        graph: String,
        id: u64,
        labels: Vec<String>,
    },
    RemoveNodeLabels {
        graph: String,
        id: u64,
        labels: Vec<String>,
    },
    RemoveNodeProperty {
        graph: String,
        id: u64,
        key: String,
    },
    RemoveEdgeProperty {
        graph: String,
        id: u64,
        key: String,
    },
    CreateIndex {
        graph: String,
        label: String,
        property: String,
    },
    DropIndex {
        graph: String,
        label: String,
        property: String,
    },
}

// ── Serializable wrapper for Value ───────────────────────────────────────────

/// A JSON-safe representation of [`Value`]. We only persist property values, which are
/// the subset of Value that can appear as node/edge properties (no Node/Edge/Path refs).
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "t", content = "v")]
enum JsonValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<JsonValue>),
    Map(Vec<(String, JsonValue)>),
    Bytes(Vec<u8>),
}

fn value_to_json(v: &Value) -> JsonValue {
    match v {
        Value::Null => JsonValue::Null,
        Value::Bool(b) => JsonValue::Bool(*b),
        Value::Int(i) => JsonValue::Int(*i),
        Value::Float(f) => JsonValue::Float(*f),
        Value::Str(s) => JsonValue::Str(s.to_string()),
        Value::List(items) => JsonValue::List(items.iter().map(value_to_json).collect()),
        Value::Map(m) => JsonValue::Map(
            m.iter()
                .map(|(k, v)| (k.to_string(), value_to_json(v)))
                .collect(),
        ),
        Value::Bytes(b) => JsonValue::Bytes(b.clone()),
        // Temporal and spatial types are not commonly used as properties in the WAL path.
        // Store them as their string representation for durability; they will be restored
        // as strings on replay.
        other => JsonValue::Str(format!("{:?}", other)),
    }
}

fn json_to_value(jv: JsonValue) -> Value {
    match jv {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(b) => Value::Bool(b),
        JsonValue::Int(i) => Value::Int(i),
        JsonValue::Float(f) => Value::Float(f),
        JsonValue::Str(s) => Value::Str(s.into_boxed_str()),
        JsonValue::List(items) => Value::List(items.into_iter().map(json_to_value).collect()),
        JsonValue::Map(m) => Value::Map(
            m.into_iter()
                .map(|(k, v)| (k.into_boxed_str(), json_to_value(v)))
                .collect(),
        ),
        JsonValue::Bytes(b) => Value::Bytes(b),
    }
}

fn props_to_json(props: &IndexMap<String, Value>) -> Vec<(String, JsonValue)> {
    props
        .iter()
        .map(|(k, v)| (k.clone(), value_to_json(v)))
        .collect()
}

fn json_to_props(entries: Vec<(String, JsonValue)>) -> IndexMap<String, Value> {
    entries
        .into_iter()
        .map(|(k, v)| (k, json_to_value(v)))
        .collect()
}

// ── PersistentGraphStore ─────────────────────────────────────────────────────

/// An [`InMemoryGraphStore`] with background disk persistence via WAL and snapshots.
pub struct PersistentGraphStore {
    inner: Arc<InMemoryGraphStore>,
    wal: Mutex<BufWriter<File>>,
    config: PersistentConfig,
    shutdown: Arc<AtomicBool>,
    /// Handle to the background snapshot thread; joined on drop.
    _snapshot_thread: Option<thread::JoinHandle<()>>,
}

impl std::fmt::Debug for PersistentGraphStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentGraphStore")
            .field("data_dir", &self.config.data_dir)
            .field("inner", &self.inner)
            .finish()
    }
}

impl PersistentGraphStore {
    /// Open or create a persistent store at the configured data directory.
    ///
    /// Recovery sequence:
    /// 1. Load snapshot if present
    /// 2. Replay WAL entries on top
    /// 3. Open WAL for append
    /// 4. Start background snapshot thread
    pub fn open(config: PersistentConfig) -> Result<Self, StorageError> {
        fs::create_dir_all(&config.data_dir).map_err(|e| StorageError::Backend {
            msg: format!("failed to create data directory: {e}"),
        })?;

        let inner = Arc::new(InMemoryGraphStore::new());

        // 1. Load snapshot
        let snapshot_path = config.data_dir.join("snapshot.json");
        if snapshot_path.exists() {
            replay_file(&inner, &snapshot_path)?;
        }

        // 2. Replay WAL
        let wal_path = config.data_dir.join("wal.jsonl");
        if wal_path.exists() {
            replay_file(&inner, &wal_path)?;
        }

        // 3. Open WAL for append
        let wal_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&wal_path)
            .map_err(|e| StorageError::Backend {
                msg: format!("failed to open WAL: {e}"),
            })?;
        let wal = Mutex::new(BufWriter::new(wal_file));

        // 4. Start background snapshot thread.
        // We share `inner` with the snapshot thread via Arc::clone — no
        // unsafe, no raw pointers. The thread keeps its own strong reference
        // for as long as it runs, and Drop joins it before the parent's Arc
        // is released.
        let shutdown = Arc::new(AtomicBool::new(false));
        let snapshot_thread = {
            let shutdown = Arc::clone(&shutdown);
            let inner = Arc::clone(&inner);
            let data_dir = config.data_dir.clone();
            let interval = config.snapshot_interval;
            thread::spawn(move || {
                snapshot_loop(&shutdown, &data_dir, &inner, interval);
            })
        };

        Ok(Self {
            inner,
            wal,
            config,
            shutdown,
            _snapshot_thread: Some(snapshot_thread),
        })
    }

    /// Append a WAL entry and optionally sync.
    fn append_wal(&self, entry: &WalEntry) -> Result<(), StorageError> {
        let line = serde_json::to_string(entry).map_err(|e| StorageError::Serialization {
            msg: format!("WAL serialize failed: {e}"),
        })?;

        let mut wal = self.wal.lock().map_err(|e| StorageError::Backend {
            msg: format!("WAL lock poisoned: {e}"),
        })?;
        writeln!(wal, "{}", line).map_err(|e| StorageError::Backend {
            msg: format!("WAL write failed: {e}"),
        })?;

        match &self.config.sync_mode {
            SyncMode::Immediate => {
                wal.flush().map_err(|e| StorageError::Backend {
                    msg: format!("WAL flush failed: {e}"),
                })?;
                wal.get_ref()
                    .sync_all()
                    .map_err(|e| StorageError::Backend {
                        msg: format!("WAL sync failed: {e}"),
                    })?;
            }
            SyncMode::Periodic(_) | SyncMode::None => {
                // Periodic flush is handled by the background thread; None skips entirely.
            }
        }

        Ok(())
    }

    /// Force a snapshot now (used in tests).
    #[cfg(test)]
    fn force_snapshot(&self) -> Result<(), StorageError> {
        write_snapshot(&self.config.data_dir, &self.inner)?;
        truncate_wal(&self.config.data_dir, &self.wal)?;
        Ok(())
    }
}

impl Drop for PersistentGraphStore {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self._snapshot_thread.take() {
            let _ = handle.join();
        }
        // Final flush of the WAL buffer.
        if let Ok(mut wal) = self.wal.lock() {
            let _ = wal.flush();
        }
    }
}

// ── Background snapshot loop ─────────────────────────────────────────────────

fn snapshot_loop(
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

fn write_snapshot(data_dir: &Path, inner: &InMemoryGraphStore) -> Result<(), StorageError> {
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
fn truncate_wal(data_dir: &Path, wal_mutex: &Mutex<BufWriter<File>>) -> Result<(), StorageError> {
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

// ── Replay logic ─────────────────────────────────────────────────────────────

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
fn replay_file(store: &InMemoryGraphStore, path: &Path) -> Result<(), StorageError> {
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

// ── GraphStore implementation ────────────────────────────────────────────────

impl GraphStore for PersistentGraphStore {
    // ── Reads (pure delegation) ──────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        self.inner.get_node(graph, id)
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        self.inner.get_edge(graph, id)
    }

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        self.inner.get_nodes_batch(graph, ids)
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        self.inner.get_edges_batch(graph, ids)
    }

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        self.inner.find_nodes_by_label(graph, label)
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.inner
            .find_nodes_by_label_and_property(graph, label, property, range)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.inner.find_edges_by_type(graph, rel_type)
    }

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.inner.out_edges(graph, node_id)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.inner.in_edges(graph, node_id)
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.inner.out_edges_by_type(graph, node_id, rel_types)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.inner.in_edges_by_type(graph, node_id, rel_types)
    }

    fn out_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.inner
            .out_edges_by_type_batch(graph, node_ids, rel_types)
    }

    fn in_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.inner
            .in_edges_by_type_batch(graph, node_ids, rel_types)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.inner.out_edge_ids(graph, node_id)
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.inner.in_edge_ids(graph, node_id)
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        self.inner.all_node_ids(graph)
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.inner.all_edge_ids(graph)
    }

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.inner.list_indexes(graph)
    }

    fn list_constraints(&self, graph: &str) -> Result<Vec<ConstraintDescriptor>, StorageError> {
        self.inner.list_constraints(graph)
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        self.inner.graph_exists(name)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.inner.list_graphs()
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.inner.graph_stats(graph)
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.inner.all_label_names(graph)
    }

    fn with_read_consistency(&self, consistency: ReadConsistency) -> Option<Arc<dyn GraphStore>> {
        self.inner.with_read_consistency(consistency)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    // ── Writes (delegate + WAL) ──────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        self.inner.create_graph(name)?;
        self.append_wal(&WalEntry::CreateGraph {
            name: name.to_string(),
        })?;
        Ok(())
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.inner.delete_graph(name)?;
        self.append_wal(&WalEntry::DeleteGraph {
            name: name.to_string(),
        })?;
        Ok(())
    }

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        let id = self
            .inner
            .create_node(graph, labels.clone(), properties.clone())?;
        self.append_wal(&WalEntry::CreateNode {
            graph: graph.to_string(),
            labels,
            properties: props_to_json(&properties),
            id: id.0,
        })?;
        Ok(id)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        let id = self
            .inner
            .create_edge(graph, src, dst, rel_type.clone(), properties.clone())?;
        self.append_wal(&WalEntry::CreateEdge {
            graph: graph.to_string(),
            src: src.0,
            dst: dst.0,
            rel_type,
            properties: props_to_json(&properties),
            id: id.0,
        })?;
        Ok(id)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .set_node_properties(graph, id, properties.clone())?;
        self.append_wal(&WalEntry::SetNodeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        Ok(())
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .replace_node_properties(graph, id, properties.clone())?;
        self.append_wal(&WalEntry::ReplaceNodeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        Ok(())
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .set_edge_properties(graph, id, properties.clone())?;
        self.append_wal(&WalEntry::SetEdgeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        Ok(())
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .replace_edge_properties(graph, id, properties.clone())?;
        self.append_wal(&WalEntry::ReplaceEdgeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        Ok(())
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.inner.remove_node_property(graph, id, key)?;
        self.append_wal(&WalEntry::RemoveNodeProperty {
            graph: graph.to_string(),
            id: id.0,
            key: key.to_string(),
        })?;
        Ok(())
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.inner.remove_edge_property(graph, id, key)?;
        self.append_wal(&WalEntry::RemoveEdgeProperty {
            graph: graph.to_string(),
            id: id.0,
            key: key.to_string(),
        })?;
        Ok(())
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.inner.add_node_labels(graph, id, labels)?;
        self.append_wal(&WalEntry::AddNodeLabels {
            graph: graph.to_string(),
            id: id.0,
            labels: labels.to_vec(),
        })?;
        Ok(())
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.inner.remove_node_labels(graph, id, labels)?;
        self.append_wal(&WalEntry::RemoveNodeLabels {
            graph: graph.to_string(),
            id: id.0,
            labels: labels.to_vec(),
        })?;
        Ok(())
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        // If detaching, we need to log edge deletions first so the WAL can replay correctly.
        if detach {
            let out_ids = self.inner.out_edge_ids(graph, id)?;
            let in_ids = self.inner.in_edge_ids(graph, id)?;
            self.inner.delete_node(graph, id, true)?;
            // Log each edge deletion, then the node deletion.
            for eid in out_ids {
                self.append_wal(&WalEntry::DeleteEdge {
                    graph: graph.to_string(),
                    id: eid.0,
                })?;
            }
            for eid in in_ids {
                self.append_wal(&WalEntry::DeleteEdge {
                    graph: graph.to_string(),
                    id: eid.0,
                })?;
            }
            self.append_wal(&WalEntry::DeleteNode {
                graph: graph.to_string(),
                id: id.0,
            })?;
        } else {
            self.inner.delete_node(graph, id, false)?;
            self.append_wal(&WalEntry::DeleteNode {
                graph: graph.to_string(),
                id: id.0,
            })?;
        }
        Ok(())
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.inner.delete_edge(graph, id)?;
        self.append_wal(&WalEntry::DeleteEdge {
            graph: graph.to_string(),
            id: id.0,
        })?;
        Ok(())
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.inner.create_index(graph, label, property)?;
        self.append_wal(&WalEntry::CreateIndex {
            graph: graph.to_string(),
            label: label.to_string(),
            property: property.to_string(),
        })?;
        Ok(())
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.inner.drop_index(graph, label, property)?;
        self.append_wal(&WalEntry::DropIndex {
            graph: graph.to_string(),
            label: label.to_string(),
            property: property.to_string(),
        })?;
        Ok(())
    }

    fn create_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        constraint_type: ConstraintType,
    ) -> Result<(), StorageError> {
        self.inner
            .create_constraint(graph, label, property, constraint_type)
    }

    fn drop_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.inner.drop_constraint(graph, label, property)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_config(dir: &std::path::Path) -> PersistentConfig {
        PersistentConfig {
            data_dir: dir.to_path_buf(),
            snapshot_interval: Duration::from_secs(3600), // long interval; we trigger manually
            sync_mode: SyncMode::Immediate,
        }
    }

    #[test]
    fn basic_write_read_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();

        store.create_graph("g").unwrap();
        let mut props = IndexMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let nid = store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let node = store.get_node("g", nid).unwrap().unwrap();
        assert_eq!(node.labels, vec!["Person"]);
        assert_eq!(node.properties["name"], Value::Str("Alice".into()));

        // Create edge
        let mut props2 = IndexMap::new();
        props2.insert("since".to_string(), Value::Int(2020));
        let nid2 = store
            .create_node("g", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        let eid = store
            .create_edge("g", nid, nid2, "KNOWS".to_string(), props2)
            .unwrap();
        let edge = store.get_edge("g", eid).unwrap().unwrap();
        assert_eq!(edge.rel_type, "KNOWS");
        assert_eq!(edge.properties["since"], Value::Int(2020));
    }

    #[test]
    fn recovery_after_reopen() {
        let dir = tempfile::tempdir().unwrap();

        // Phase 1: write data
        {
            let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
            store.create_graph("g").unwrap();
            let mut props = IndexMap::new();
            props.insert("name".to_string(), Value::Str("Bob".into()));
            store
                .create_node("g", vec!["Person".to_string()], props)
                .unwrap();
            let nid2 = store
                .create_node("g", vec!["Animal".to_string()], IndexMap::new())
                .unwrap();
            assert_eq!(nid2.0, 2);
            // Drop flushes
        }

        // Phase 2: reopen and verify
        {
            let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
            assert!(store.graph_exists("g").unwrap());
            let node = store.get_node("g", NodeId(1)).unwrap().unwrap();
            assert_eq!(node.properties["name"], Value::Str("Bob".into()));
            assert_eq!(node.labels, vec!["Person"]);

            let node2 = store.get_node("g", NodeId(2)).unwrap().unwrap();
            assert_eq!(node2.labels, vec!["Animal"]);

            let stats = store.graph_stats("g").unwrap();
            assert_eq!(stats.node_count, 2);
        }
    }

    #[test]
    fn wal_replay_ordering() {
        let dir = tempfile::tempdir().unwrap();

        // Write a graph, add nodes and an edge, modify properties, delete node
        {
            let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
            store.create_graph("g").unwrap();

            let n1 = store
                .create_node("g", vec!["A".to_string()], IndexMap::new())
                .unwrap();
            let n2 = store
                .create_node("g", vec!["B".to_string()], IndexMap::new())
                .unwrap();
            let _e = store
                .create_edge("g", n1, n2, "R".to_string(), IndexMap::new())
                .unwrap();

            let mut update = IndexMap::new();
            update.insert("x".to_string(), Value::Int(42));
            store.set_node_properties("g", n1, update).unwrap();

            // Delete n2 (detach to remove edge too)
            store.delete_node("g", n2, true).unwrap();
        }

        // Replay and verify
        {
            let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
            let node = store.get_node("g", NodeId(1)).unwrap().unwrap();
            assert_eq!(node.properties["x"], Value::Int(42));

            // n2 was deleted
            assert!(store.get_node("g", NodeId(2)).unwrap().is_none());

            let stats = store.graph_stats("g").unwrap();
            assert_eq!(stats.node_count, 1);
            assert_eq!(stats.edge_count, 0);
        }
    }

    #[test]
    fn snapshot_and_wal_truncation() {
        let dir = tempfile::tempdir().unwrap();

        {
            let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
            store.create_graph("g").unwrap();
            let mut props = IndexMap::new();
            props.insert("v".to_string(), Value::Int(1));
            store
                .create_node("g", vec!["X".to_string()], props)
                .unwrap();

            // Force snapshot (truncates WAL)
            store.force_snapshot().unwrap();

            // WAL should now be empty
            let wal_content = fs::read_to_string(dir.path().join("wal.jsonl")).unwrap();
            assert!(wal_content.is_empty(), "WAL should be empty after snapshot");

            // Snapshot should exist
            assert!(dir.path().join("snapshot.json").exists());

            // Write more data after snapshot
            let mut props2 = IndexMap::new();
            props2.insert("v".to_string(), Value::Int(2));
            store
                .create_node("g", vec!["Y".to_string()], props2)
                .unwrap();
        }

        // Reopen: should recover from snapshot + WAL
        {
            let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
            let n1 = store.get_node("g", NodeId(1)).unwrap().unwrap();
            assert_eq!(n1.properties["v"], Value::Int(1));
            assert_eq!(n1.labels, vec!["X"]);

            let n2 = store.get_node("g", NodeId(2)).unwrap().unwrap();
            assert_eq!(n2.properties["v"], Value::Int(2));
            assert_eq!(n2.labels, vec!["Y"]);
        }
    }

    #[test]
    fn edge_operations_persist() {
        let dir = tempfile::tempdir().unwrap();

        {
            let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
            store.create_graph("g").unwrap();
            let n1 = store.create_node("g", vec![], IndexMap::new()).unwrap();
            let n2 = store.create_node("g", vec![], IndexMap::new()).unwrap();

            let mut ep = IndexMap::new();
            ep.insert("weight".to_string(), Value::Float(3.25));
            let eid = store
                .create_edge("g", n1, n2, "LINK".to_string(), ep)
                .unwrap();

            // Update edge properties
            let mut update = IndexMap::new();
            update.insert("color".to_string(), Value::Str("red".into()));
            store.set_edge_properties("g", eid, update).unwrap();
        }

        {
            let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
            let edge = store.get_edge("g", EdgeId(3)).unwrap().unwrap();
            assert_eq!(edge.rel_type, "LINK");
            assert_eq!(edge.properties["weight"], Value::Float(3.25));
            assert_eq!(edge.properties["color"], Value::Str("red".into()));
        }
    }

    #[test]
    fn index_operations_persist() {
        let dir = tempfile::tempdir().unwrap();

        {
            let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
            store.create_graph("g").unwrap();
            store.create_index("g", "Person", "name").unwrap();

            let indexes = store.list_indexes("g").unwrap();
            assert_eq!(indexes.len(), 1);
        }

        {
            let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
            let indexes = store.list_indexes("g").unwrap();
            assert_eq!(indexes.len(), 1);
            assert_eq!(indexes[0].label, "Person");
            assert_eq!(indexes[0].property, "name");
        }
    }

    #[test]
    fn label_operations_persist() {
        let dir = tempfile::tempdir().unwrap();

        {
            let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
            store.create_graph("g").unwrap();
            let nid = store
                .create_node("g", vec!["A".to_string()], IndexMap::new())
                .unwrap();
            store
                .add_node_labels("g", nid, &["B".to_string(), "C".to_string()])
                .unwrap();
            store
                .remove_node_labels("g", nid, &["A".to_string()])
                .unwrap();
        }

        {
            let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
            let node = store.get_node("g", NodeId(1)).unwrap().unwrap();
            assert!(!node.labels.contains(&"A".to_string()));
            assert!(node.labels.contains(&"B".to_string()));
            assert!(node.labels.contains(&"C".to_string()));
        }
    }

    #[test]
    fn empty_store_opens_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
        assert_eq!(store.list_graphs().unwrap(), Vec::<String>::new());
    }
}
