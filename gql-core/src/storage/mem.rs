//! In-memory graph store — primary implementation for unit tests, CLI, and Phase 0.
//!
//! All data lives in a single `RwLock<Inner>`. Reads acquire a shared lock; writes acquire
//! an exclusive lock. There are no sub-graph locks; the store is optimised for correctness
//! and test velocity, not raw throughput (the Redis backend handles production load).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, RwLock};

use crate::error::StorageError;
use crate::storage::{
    validate_graph_name, AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery,
};
use crate::types::Value;

// ── IndexableValue: Ord-compatible wrapper for BTreeMap keys ──────────────────

/// A value that can be used as a BTreeMap key for property indexes.
/// Wraps the subset of `Value` variants that support total ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
enum IndexableValue {
    Bool(bool),
    Int(i64),
    /// f64 stored as raw bits for Ord. NaN values are excluded at insertion time.
    Float(u64),
    Str(Box<str>),
}

impl IndexableValue {
    /// Convert a `Value` to an `IndexableValue`, returning `None` for non-indexable types.
    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Bool(b) => Some(IndexableValue::Bool(*b)),
            Value::Int(i) => Some(IndexableValue::Int(*i)),
            Value::Float(f) => {
                if f.is_nan() {
                    None
                } else {
                    Some(IndexableValue::Float(f.to_bits()))
                }
            }
            Value::Str(s) => Some(IndexableValue::Str(s.clone())),
            _ => None,
        }
    }
}

impl PartialOrd for IndexableValue {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for IndexableValue {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Different types are ordered by discriminant to keep BTreeMap consistent.
        let disc = |v: &IndexableValue| -> u8 {
            match v {
                IndexableValue::Bool(_) => 0,
                IndexableValue::Int(_) => 1,
                IndexableValue::Float(_) => 2,
                IndexableValue::Str(_) => 3,
            }
        };
        let d1 = disc(self);
        let d2 = disc(other);
        if d1 != d2 {
            return d1.cmp(&d2);
        }
        match (self, other) {
            (IndexableValue::Bool(a), IndexableValue::Bool(b)) => a.cmp(b),
            (IndexableValue::Int(a), IndexableValue::Int(b)) => a.cmp(b),
            (IndexableValue::Float(a), IndexableValue::Float(b)) => {
                // Both are non-NaN, use total_cmp semantics via bits
                let fa = f64::from_bits(*a);
                let fb = f64::from_bits(*b);
                fa.total_cmp(&fb)
            }
            (IndexableValue::Str(a), IndexableValue::Str(b)) => a.cmp(b),
            _ => unreachable!(),
        }
    }
}

/// Composite key for the property index BTreeMap: (label, property, value).
type PropIndexKey = (String, String, IndexableValue);

/// The property index: maps (label, property, value) -> set of raw node IDs.
type PropIndex = BTreeMap<PropIndexKey, HashSet<u64>>;

// ── Per-graph state ────────────────────────────────────────────────────────────

/// All mutable state for a single named graph.
#[derive(Clone)]
pub(crate) struct GraphInner {
    pub(crate) nodes: HashMap<u64, NodeData>,
    pub(crate) edges: HashMap<u64, EdgeData>,
    /// Outgoing adjacency: node_id -> list of (edge_id, neighbor_id, rel_type)
    out_adj: HashMap<u64, Vec<AdjEntry>>,
    /// Incoming adjacency: node_id -> list of (edge_id, neighbor_id, rel_type)
    in_adj: HashMap<u64, Vec<AdjEntry>>,
    /// label -> set of node IDs
    label_index: HashMap<String, HashSet<u64>>,
    /// rel_type -> set of edge IDs
    reltype_index: HashMap<String, HashSet<u64>>,
    indexes: Vec<IndexDescriptor>,
    /// BTreeMap-based property indexes: (label, property, value) -> node IDs.
    prop_index: PropIndex,
    node_seq: u64,
    edge_seq: u64,
}

impl GraphInner {
    fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
            out_adj: HashMap::new(),
            in_adj: HashMap::new(),
            label_index: HashMap::new(),
            reltype_index: HashMap::new(),
            indexes: Vec::new(),
            prop_index: BTreeMap::new(),
            node_seq: 0,
            edge_seq: 0,
        }
    }

    /// Allocate the next node ID. Returns `Err` on u64 overflow (astronomically unlikely).
    fn next_node_id(&mut self, graph_name: &str) -> Result<u64, StorageError> {
        self.node_seq = self
            .node_seq
            .checked_add(1)
            .ok_or_else(|| StorageError::IdOverflow {
                graph: graph_name.to_string(),
            })?;
        Ok(self.node_seq)
    }

    /// Check if an index exists for the given (label, property) pair.
    fn has_index(&self, label: &str, property: &str) -> bool {
        self.indexes
            .iter()
            .any(|i| i.label == label && i.property == property)
    }

    /// Insert a node's property value into the BTreeMap index if an index exists.
    fn index_insert(&mut self, raw_id: u64, labels: &[String], props: &HashMap<String, Value>) {
        for idx in &self.indexes {
            if labels.contains(&idx.label) {
                if let Some(val) = props.get(&idx.property) {
                    if let Some(iv) = IndexableValue::from_value(val) {
                        self.prop_index
                            .entry((idx.label.clone(), idx.property.clone(), iv))
                            .or_default()
                            .insert(raw_id);
                    }
                }
            }
        }
    }

    /// Remove a node's property value from the BTreeMap index.
    fn index_remove(&mut self, raw_id: u64, labels: &[String], props: &HashMap<String, Value>) {
        for idx in &self.indexes {
            if labels.contains(&idx.label) {
                if let Some(val) = props.get(&idx.property) {
                    if let Some(iv) = IndexableValue::from_value(val) {
                        let key = (idx.label.clone(), idx.property.clone(), iv);
                        if let Some(set) = self.prop_index.get_mut(&key) {
                            set.remove(&raw_id);
                            if set.is_empty() {
                                self.prop_index.remove(&key);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Allocate the next edge ID.
    fn next_edge_id(&mut self, graph_name: &str) -> Result<u64, StorageError> {
        self.edge_seq = self
            .edge_seq
            .checked_add(1)
            .ok_or_else(|| StorageError::IdOverflow {
                graph: graph_name.to_string(),
            })?;
        Ok(self.edge_seq)
    }
}

// ── Store ─────────────────────────────────────────────────────────────────────

/// Thread-safe in-memory implementation of [`GraphStore`].
///
/// Uses an `Arc`-based copy-on-write approach for snapshot isolation: the inner
/// state is wrapped in `Arc` so readers can clone the `Arc` (a cheap refcount
/// bump) to obtain a frozen snapshot that is immune to concurrent writes.
/// Writers call `Arc::make_mut` which clones the data only when other references
/// exist, providing copy-on-write semantics.
pub struct InMemoryGraphStore {
    /// map: graph_name -> per-graph state, wrapped in Arc for COW snapshots.
    inner: RwLock<Arc<HashMap<String, GraphInner>>>,
}

/// A frozen, read-only snapshot of the store at a point in time.
///
/// Obtained via [`InMemoryGraphStore::snapshot_handle`]. All reads through
/// this handle see a consistent view even if the underlying store is
/// mutated concurrently.
#[allow(dead_code)]
pub(crate) struct Snapshot {
    data: Arc<HashMap<String, GraphInner>>,
}

impl Snapshot {
    /// Look up a graph by name.
    #[allow(dead_code)]
    pub(crate) fn get_graph(&self, name: &str) -> Option<&GraphInner> {
        self.data.get(name)
    }
}

impl std::fmt::Debug for InMemoryGraphStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let graph_names: Vec<String> = self
            .inner
            .read()
            .map(|g| g.keys().cloned().collect())
            .unwrap_or_default();
        f.debug_struct("InMemoryGraphStore")
            .field("graphs", &graph_names)
            .finish()
    }
}

impl InMemoryGraphStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Arc::new(HashMap::new())),
        }
    }

    /// Acquire a read snapshot: clones the `Arc` (cheap refcount bump) and
    /// immediately releases the lock. The returned `Arc` is a frozen view.
    fn read_snapshot(&self) -> Result<Arc<HashMap<String, GraphInner>>, StorageError> {
        let guard = self
            .inner
            .read()
            .map_err(|e| StorageError::backend(format!("lock poisoned (read): {e}")))?;
        Ok(Arc::clone(&guard))
    }

    /// Acquire a write lock.
    fn write(
        &self,
    ) -> Result<std::sync::RwLockWriteGuard<'_, Arc<HashMap<String, GraphInner>>>, StorageError>
    {
        self.inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned (write): {e}")))
    }

    /// Take a consistent, multi-read snapshot that can be held across
    /// multiple operations without being affected by concurrent writes.
    #[allow(dead_code)]
    pub(crate) fn snapshot_handle(&self) -> Result<Snapshot, StorageError> {
        Ok(Snapshot {
            data: self.read_snapshot()?,
        })
    }

    /// Create a snapshot containing deep copies of the specified graphs.
    ///
    /// The snapshot is an independent `InMemoryGraphStore` that can be mutated
    /// without affecting the original. Graphs that do not exist in this store
    /// are silently skipped.
    pub fn snapshot(&self, graphs: &[&str]) -> Result<InMemoryGraphStore, StorageError> {
        let snap = self.read_snapshot()?;
        let mut snapshot_inner = HashMap::new();
        for &name in graphs {
            if let Some(graph) = snap.get(name) {
                snapshot_inner.insert(name.to_string(), graph.clone());
            }
        }
        Ok(InMemoryGraphStore {
            inner: RwLock::new(Arc::new(snapshot_inner)),
        })
    }

    /// Merge all graphs from `other` into this store, replacing existing graphs.
    ///
    /// Used to promote a snapshot on COMMIT: every graph in the snapshot
    /// overwrites the corresponding graph in the main store.
    pub fn merge_from(&self, other: &InMemoryGraphStore) -> Result<(), StorageError> {
        let other_snap = other.read_snapshot()?;
        let mut self_guard = self.write()?;
        let map = Arc::make_mut(&mut self_guard);
        for (name, graph) in other_snap.iter() {
            map.insert(name.clone(), graph.clone());
        }
        Ok(())
    }

    /// Helper: get a reference to a graph or return `GraphNotFound`.
    fn require_graph<'g>(
        map: &'g HashMap<String, GraphInner>,
        graph: &str,
    ) -> Result<&'g GraphInner, StorageError> {
        map.get(graph).ok_or_else(|| StorageError::GraphNotFound {
            name: graph.to_string(),
        })
    }

    /// Helper: get a mutable reference to a graph or return `GraphNotFound`.
    fn require_graph_mut<'g>(
        map: &'g mut HashMap<String, GraphInner>,
        graph: &str,
    ) -> Result<&'g mut GraphInner, StorageError> {
        map.get_mut(graph)
            .ok_or_else(|| StorageError::GraphNotFound {
                name: graph.to_string(),
            })
    }
}

impl Default for InMemoryGraphStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── Value comparison helpers for RangeQuery ───────────────────────────────────

/// Total order for Value for range queries.  Returns `None` when the values are
/// incomparable (e.g. different types, or either is Null).
fn value_partial_cmp(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.partial_cmp(y),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y),
        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y),
        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)),
        (Value::Str(x), Value::Str(y)) => Some(x.as_ref().cmp(y.as_ref())),
        (Value::Bool(x), Value::Bool(y)) => x.partial_cmp(y),
        _ => None,
    }
}

fn matches_range(prop_val: &Value, range: &RangeQuery<'_>) -> bool {
    match range {
        RangeQuery::Eq(target) => prop_val.cypher_eq(target).unwrap_or(false),
        RangeQuery::Lt(bound) => {
            matches!(
                value_partial_cmp(prop_val, bound),
                Some(std::cmp::Ordering::Less)
            )
        }
        RangeQuery::Lte(bound) => matches!(
            value_partial_cmp(prop_val, bound),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ),
        RangeQuery::Gt(bound) => {
            matches!(
                value_partial_cmp(prop_val, bound),
                Some(std::cmp::Ordering::Greater)
            )
        }
        RangeQuery::Gte(bound) => matches!(
            value_partial_cmp(prop_val, bound),
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ),
        RangeQuery::Between { lo, hi, inclusive } => {
            let ge_lo = matches!(
                value_partial_cmp(prop_val, lo),
                Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
            );
            if !ge_lo {
                return false;
            }
            if *inclusive {
                matches!(
                    value_partial_cmp(prop_val, hi),
                    Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                )
            } else {
                matches!(
                    value_partial_cmp(prop_val, hi),
                    Some(std::cmp::Ordering::Less)
                )
            }
        }
        RangeQuery::StartsWith(prefix) => {
            if let Value::Str(s) = prop_val {
                s.starts_with(prefix)
            } else {
                false
            }
        }
    }
}

// ── Indexed range query helpers ───────────────────────────────────────────────

/// Collect node IDs from the BTreeMap property index that match the given range query.
fn indexed_range_query(
    prop_index: &PropIndex,
    label: &str,
    property: &str,
    range: &RangeQuery<'_>,
) -> Vec<NodeId> {
    use std::ops::Bound;

    let l = label.to_string();
    let p = property.to_string();

    // Helper: collect all node IDs from a BTreeMap range.
    macro_rules! collect_range {
        ($lo:expr, $hi:expr) => {{
            let mut result = Vec::new();
            for ((_l, _p, _v), ids) in prop_index.range(($lo, $hi)) {
                if _l != &l || _p != &p {
                    continue;
                }
                for &raw_id in ids {
                    result.push(NodeId(raw_id));
                }
            }
            result
        }};
    }

    // The smallest possible key for this (label, property).
    let min_key = (l.clone(), p.clone(), IndexableValue::Bool(false));

    match range {
        RangeQuery::Eq(target) => {
            if let Some(iv) = IndexableValue::from_value(target) {
                let key = (l, p, iv);
                prop_index
                    .get(&key)
                    .map(|ids| ids.iter().copied().map(NodeId).collect())
                    .unwrap_or_default()
            } else {
                vec![]
            }
        }
        RangeQuery::Lt(bound) => {
            if let Some(iv) = IndexableValue::from_value(bound) {
                let lo = Bound::Included(min_key);
                let hi = Bound::Excluded((l.clone(), p.clone(), iv));
                collect_range!(lo, hi)
            } else {
                vec![]
            }
        }
        RangeQuery::Lte(bound) => {
            if let Some(iv) = IndexableValue::from_value(bound) {
                let lo = Bound::Included(min_key);
                let hi = Bound::Included((l.clone(), p.clone(), iv));
                collect_range!(lo, hi)
            } else {
                vec![]
            }
        }
        RangeQuery::Gt(bound) => {
            if let Some(iv) = IndexableValue::from_value(bound) {
                let mut result = Vec::new();
                for ((kl, kp, _kv), ids) in prop_index.range((
                    Bound::Excluded((l.clone(), p.clone(), iv)),
                    Bound::Unbounded,
                )) {
                    if kl != &l || kp != &p {
                        break;
                    }
                    for &raw_id in ids {
                        result.push(NodeId(raw_id));
                    }
                }
                result
            } else {
                vec![]
            }
        }
        RangeQuery::Gte(bound) => {
            if let Some(iv) = IndexableValue::from_value(bound) {
                let mut result = Vec::new();
                for ((kl, kp, _kv), ids) in prop_index.range((
                    Bound::Included((l.clone(), p.clone(), iv)),
                    Bound::Unbounded,
                )) {
                    if kl != &l || kp != &p {
                        break;
                    }
                    for &raw_id in ids {
                        result.push(NodeId(raw_id));
                    }
                }
                result
            } else {
                vec![]
            }
        }
        RangeQuery::Between { lo, hi, inclusive } => {
            let lo_iv = IndexableValue::from_value(lo);
            let hi_iv = IndexableValue::from_value(hi);
            match (lo_iv, hi_iv) {
                (Some(lo_v), Some(hi_v)) => {
                    let lo_bound = Bound::Included((l.clone(), p.clone(), lo_v));
                    let hi_bound = if *inclusive {
                        Bound::Included((l.clone(), p.clone(), hi_v))
                    } else {
                        Bound::Excluded((l.clone(), p.clone(), hi_v))
                    };
                    collect_range!(lo_bound, hi_bound)
                }
                _ => vec![],
            }
        }
        RangeQuery::StartsWith(prefix) => {
            // Scan string range: from prefix to prefix with last byte incremented.
            let lo_v = IndexableValue::Str((*prefix).into());
            let mut result = Vec::new();
            for ((kl, kp, kv), ids) in prop_index.range((
                Bound::Included((l.clone(), p.clone(), lo_v)),
                Bound::Unbounded,
            )) {
                if kl != &l || kp != &p {
                    break;
                }
                if let IndexableValue::Str(s) = kv {
                    if s.starts_with(prefix) {
                        for &raw_id in ids {
                            result.push(NodeId(raw_id));
                        }
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            result
        }
    }
}

// ── GraphStore impl ───────────────────────────────────────────────────────────

impl GraphStore for InMemoryGraphStore {
    // ── Single-item reads ──────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(g.nodes.get(&id.0).cloned())
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(g.edges.get(&id.0).cloned())
    }

    // ── Batch reads ────────────────────────────────────────────────────────────

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(ids.iter().map(|id| g.nodes.get(&id.0).cloned()).collect())
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(ids.iter().map(|id| g.edges.get(&id.0).cloned()).collect())
    }

    // ── Label/type index reads ─────────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        let ids = g
            .label_index
            .get(label)
            .map(|s| s.iter().copied().map(NodeId).collect())
            .unwrap_or_default();
        Ok(ids)
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;

        // Use BTreeMap index if available.
        if g.has_index(label, property) {
            return Ok(indexed_range_query(&g.prop_index, label, property, &range));
        }

        // Fallback: full scan over label set.
        let candidate_ids = match g.label_index.get(label) {
            Some(s) => s,
            None => return Ok(vec![]),
        };
        let mut result = Vec::new();
        for &raw_id in candidate_ids {
            if let Some(node) = g.nodes.get(&raw_id) {
                if let Some(prop_val) = node.properties.get(property) {
                    if matches_range(prop_val, &range) {
                        result.push(NodeId(raw_id));
                    }
                }
            }
        }
        Ok(result)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        let ids = g
            .reltype_index
            .get(rel_type)
            .map(|s| s.iter().copied().map(EdgeId).collect())
            .unwrap_or_default();
        Ok(ids)
    }

    // ── Adjacency reads ────────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(g.out_adj.get(&node_id.0).cloned().unwrap_or_default())
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(g.in_adj.get(&node_id.0).cloned().unwrap_or_default())
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        let all = g.out_adj.get(&node_id.0).cloned().unwrap_or_default();
        if rel_types.is_empty() {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|e| rel_types.contains(&e.rel_type.as_str()))
            .collect())
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        let all = g.in_adj.get(&node_id.0).cloned().unwrap_or_default();
        if rel_types.is_empty() {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|e| rel_types.contains(&e.rel_type.as_str()))
            .collect())
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(g.out_adj
            .get(&node_id.0)
            .map(|v| v.iter().map(|e| e.edge_id).collect())
            .unwrap_or_default())
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(g.in_adj
            .get(&node_id.0)
            .map(|v| v.iter().map(|e| e.edge_id).collect())
            .unwrap_or_default())
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(g.nodes.keys().copied().map(NodeId).collect())
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(g.edges.keys().copied().map(EdgeId).collect())
    }

    // ── Write operations ───────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: HashMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;

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
        clean_props.remove("_sd2_id");
        clean_props.remove("_sd2_eid");

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

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: HashMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;
        // Validate that both endpoints exist.
        if !g.nodes.contains_key(&src.0) {
            return Err(StorageError::NodeNotFound { id: src.0 });
        }
        if !g.nodes.contains_key(&dst.0) {
            return Err(StorageError::NodeNotFound { id: dst.0 });
        }

        // Honor pre-set ID from FanoutStore injection
        let raw_id = if let Some(Value::Int(i)) = properties.get("_sd2_eid") {
            if *i < 0 {
                return Err(StorageError::Backend {
                    msg: format!("_sd2_eid must be non-negative, got {i}"),
                });
            }
            *i as u64
        } else {
            g.next_edge_id(graph)?
        };
        let edge_id = EdgeId(raw_id);

        // Strip internal keys before storing
        let mut clean_props = properties;
        clean_props.remove("_sd2_id");
        clean_props.remove("_sd2_eid");

        let edge = EdgeData {
            id: edge_id,
            rel_type: rel_type.clone(),
            src,
            dst,
            properties: clean_props,
        };
        g.edges.insert(raw_id, edge);

        // Update adjacency lists.
        g.out_adj.entry(src.0).or_default().push(AdjEntry {
            edge_id,
            neighbor_id: dst,
            rel_type: rel_type.clone(),
        });
        g.in_adj.entry(dst.0).or_default().push(AdjEntry {
            edge_id,
            neighbor_id: src,
            rel_type: rel_type.clone(),
        });

        // Update rel-type index.
        g.reltype_index.entry(rel_type).or_default().insert(raw_id);
        Ok(edge_id)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;
        let node = g
            .nodes
            .get_mut(&id.0)
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        // Remove old index entries for changed keys, then insert new ones.
        let labels = node.labels.clone();
        let old_props: HashMap<String, Value> = properties
            .keys()
            .filter_map(|k| node.properties.get(k).map(|v| (k.clone(), v.clone())))
            .collect();
        g.index_remove(id.0, &labels, &old_props);
        // Merge: overwrite keys present in `properties`, leave others alone.
        let node = g.nodes.get_mut(&id.0).unwrap();
        for (k, v) in &properties {
            node.properties.insert(k.clone(), v.clone());
        }
        g.index_insert(id.0, &labels, &properties);
        Ok(())
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;
        let node = g
            .nodes
            .get_mut(&id.0)
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let labels = node.labels.clone();
        let old_props = node.properties.clone();
        g.index_remove(id.0, &labels, &old_props);
        let node = g.nodes.get_mut(&id.0).unwrap();
        node.properties = properties.clone();
        g.index_insert(id.0, &labels, &properties);
        Ok(())
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;
        let edge = g
            .edges
            .get_mut(&id.0)
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        for (k, v) in properties {
            edge.properties.insert(k, v);
        }
        Ok(())
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;
        let edge = g
            .edges
            .get_mut(&id.0)
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        edge.properties = properties;
        Ok(())
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;
        let node = g
            .nodes
            .get_mut(&id.0)
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        if let Some(old_val) = node.properties.remove(key) {
            let labels = node.labels.clone();
            let old_props = HashMap::from([(key.to_string(), old_val)]);
            g.index_remove(id.0, &labels, &old_props);
        }
        Ok(())
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;
        let edge = g
            .edges
            .get_mut(&id.0)
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        edge.properties.remove(key);
        Ok(())
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;
        let node = g
            .nodes
            .get_mut(&id.0)
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let new_labels: Vec<String> = labels
            .iter()
            .filter(|l| !node.labels.contains(l))
            .cloned()
            .collect();
        let props = node.properties.clone();
        for label in &new_labels {
            node.labels.push(label.clone());
        }
        // Update label index.
        for label in &new_labels {
            g.label_index.entry(label.clone()).or_default().insert(id.0);
        }
        // Index new label+property combinations.
        g.index_insert(id.0, &new_labels, &props);
        Ok(())
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;
        let node = g
            .nodes
            .get_mut(&id.0)
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let props = node.properties.clone();
        for label in labels {
            node.labels.retain(|l| l != label);
        }
        // Remove from property indexes for removed labels.
        g.index_remove(id.0, labels, &props);
        // Update label index.
        for label in labels {
            if let Some(set) = g.label_index.get_mut(label) {
                set.remove(&id.0);
                if set.is_empty() {
                    g.label_index.remove(label);
                }
            }
        }
        Ok(())
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;

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

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;

        let edge = g
            .edges
            .remove(&id.0)
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;

        // Remove from adjacency lists.
        if let Some(v) = g.out_adj.get_mut(&edge.src.0) {
            v.retain(|e| e.edge_id != id);
        }
        if let Some(v) = g.in_adj.get_mut(&edge.dst.0) {
            v.retain(|e| e.edge_id != id);
        }

        // Remove from rel-type index.
        if let Some(set) = g.reltype_index.get_mut(&edge.rel_type) {
            set.remove(&id.0);
            if set.is_empty() {
                g.reltype_index.remove(&edge.rel_type);
            }
        }

        Ok(())
    }

    // ── Index management ───────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(g.indexes.clone())
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;
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

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = Self::require_graph_mut(map, graph)?;
        g.indexes
            .retain(|i| !(i.label == label && i.property == property));
        // Remove all BTreeMap entries for this label+property.
        let prefix = (label.to_string(), property.to_string());
        let keys_to_remove: Vec<PropIndexKey> = g
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

    // ── Graph administration ───────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        if map.contains_key(name) {
            return Err(StorageError::GraphAlreadyExists {
                name: name.to_string(),
            });
        }
        map.insert(name.to_string(), GraphInner::new());
        Ok(())
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        if map.remove(name).is_none() {
            return Err(StorageError::GraphNotFound {
                name: name.to_string(),
            });
        }
        Ok(())
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        let snap = self.read_snapshot()?;
        Ok(snap.contains_key(name))
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        let snap = self.read_snapshot()?;
        let mut names: Vec<String> = snap.keys().cloned().collect();
        names.sort();
        Ok(names)
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(GraphStats {
            node_count: g.nodes.len() as u64,
            edge_count: g.edges.len() as u64,
            label_count: g.label_index.len() as u32,
            rel_type_count: g.reltype_index.len() as u32,
        })
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(g.label_index.keys().cloned().collect())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn tx_snapshot(&self, graphs: &[&str]) -> Result<Option<Box<dyn GraphStore>>, StorageError> {
        let snap = self.snapshot(graphs)?;
        Ok(Some(Box::new(snap)))
    }

    fn tx_commit(&self, snapshot: &dyn GraphStore) -> Result<(), StorageError> {
        let snap = snapshot
            .as_any()
            .downcast_ref::<InMemoryGraphStore>()
            .ok_or_else(|| {
                StorageError::backend("tx_commit: snapshot is not an InMemoryGraphStore")
            })?;
        self.merge_from(snap)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> InMemoryGraphStore {
        let s = InMemoryGraphStore::new();
        s.create_graph("test").unwrap();
        s
    }

    // ── Create / read / delete roundtrip ──────────────────────────────────────

    #[test]
    fn node_create_read_delete_roundtrip() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let id = store
            .create_node("test", vec!["Person".to_string()], props.clone())
            .unwrap();

        let node = store
            .get_node("test", id)
            .unwrap()
            .expect("node should exist");
        assert_eq!(node.id, id);
        assert_eq!(node.labels, vec!["Person".to_string()]);
        assert_eq!(node.properties["name"], Value::Str("Alice".into()));

        store.delete_node("test", id, false).unwrap();
        assert!(store.get_node("test", id).unwrap().is_none());
    }

    #[test]
    fn edge_create_read_delete_roundtrip() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let mut props = HashMap::new();
        props.insert("weight".to_string(), Value::Float(1.5));
        let eid = store
            .create_edge("test", a, b, "KNOWS".to_string(), props)
            .unwrap();

        let edge = store
            .get_edge("test", eid)
            .unwrap()
            .expect("edge should exist");
        assert_eq!(edge.src, a);
        assert_eq!(edge.dst, b);
        assert_eq!(edge.rel_type, "KNOWS");
        assert_eq!(edge.properties["weight"], Value::Float(1.5));

        store.delete_edge("test", eid).unwrap();
        assert!(store.get_edge("test", eid).unwrap().is_none());
    }

    // ── Label scanning ────────────────────────────────────────────────────────

    #[test]
    fn label_scan_returns_correct_nodes() {
        let store = make_store();
        let a = store
            .create_node("test", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("test", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        let _c = store
            .create_node("test", vec!["Company".to_string()], HashMap::new())
            .unwrap();

        let mut persons = store.find_nodes_by_label("test", "Person").unwrap();
        persons.sort();
        assert_eq!(persons, vec![a, b]);

        let companies = store.find_nodes_by_label("test", "Company").unwrap();
        assert_eq!(companies.len(), 1);

        let missing = store.find_nodes_by_label("test", "DoesNotExist").unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    fn label_and_property_scan_eq() {
        let store = make_store();
        let mut p1 = HashMap::new();
        p1.insert("age".to_string(), Value::Int(30));
        let a = store
            .create_node("test", vec!["Person".to_string()], p1)
            .unwrap();

        let mut p2 = HashMap::new();
        p2.insert("age".to_string(), Value::Int(25));
        let _b = store
            .create_node("test", vec!["Person".to_string()], p2)
            .unwrap();

        let bound = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert_eq!(result, vec![a]);
    }

    #[test]
    fn label_and_property_scan_range() {
        let store = make_store();
        let make_person = |age: i64| {
            let mut p = HashMap::new();
            p.insert("age".to_string(), Value::Int(age));
            p
        };
        let a = store
            .create_node("test", vec!["Person".to_string()], make_person(20))
            .unwrap();
        let b = store
            .create_node("test", vec!["Person".to_string()], make_person(30))
            .unwrap();
        let _c = store
            .create_node("test", vec!["Person".to_string()], make_person(40))
            .unwrap();

        let lo = Value::Int(20);
        let hi = Value::Int(30);
        let mut result = store
            .find_nodes_by_label_and_property(
                "test",
                "Person",
                "age",
                RangeQuery::Between {
                    lo: &lo,
                    hi: &hi,
                    inclusive: true,
                },
            )
            .unwrap();
        result.sort();
        assert_eq!(result, vec![a, b]);
    }

    #[test]
    fn label_and_property_scan_starts_with() {
        let store = make_store();
        let mut p1 = HashMap::new();
        p1.insert("name".to_string(), Value::Str("Alice".into()));
        let a = store
            .create_node("test", vec!["Person".to_string()], p1)
            .unwrap();

        let mut p2 = HashMap::new();
        p2.insert("name".to_string(), Value::Str("Bob".into()));
        let _b = store
            .create_node("test", vec!["Person".to_string()], p2)
            .unwrap();

        let result = store
            .find_nodes_by_label_and_property(
                "test",
                "Person",
                "name",
                RangeQuery::StartsWith("Al"),
            )
            .unwrap();
        assert_eq!(result, vec![a]);
    }

    // ── Adjacency ─────────────────────────────────────────────────────────────

    #[test]
    fn adjacency_out_and_in() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let eid = store
            .create_edge("test", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        let out = store.out_edges("test", a).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].edge_id, eid);
        assert_eq!(out[0].neighbor_id, b);
        assert_eq!(out[0].rel_type, "KNOWS");

        let in_edges = store.in_edges("test", b).unwrap();
        assert_eq!(in_edges.len(), 1);
        assert_eq!(in_edges[0].edge_id, eid);
        assert_eq!(in_edges[0].neighbor_id, a);

        // a has no incoming, b has no outgoing
        assert!(store.in_edges("test", a).unwrap().is_empty());
        assert!(store.out_edges("test", b).unwrap().is_empty());
    }

    #[test]
    fn out_edges_by_type_filters() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let c = store.create_node("test", vec![], HashMap::new()).unwrap();

        store
            .create_edge("test", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("test", a, c, "LIKES".to_string(), HashMap::new())
            .unwrap();

        let knows = store.out_edges_by_type("test", a, &["KNOWS"]).unwrap();
        assert_eq!(knows.len(), 1);
        assert_eq!(knows[0].neighbor_id, b);

        let all = store.out_edges_by_type("test", a, &[]).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn in_edges_by_type_filters() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let c = store.create_node("test", vec![], HashMap::new()).unwrap();

        store
            .create_edge("test", a, c, "KNOWS".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("test", b, c, "LIKES".to_string(), HashMap::new())
            .unwrap();

        let knows_in = store.in_edges_by_type("test", c, &["KNOWS"]).unwrap();
        assert_eq!(knows_in.len(), 1);
        assert_eq!(knows_in[0].neighbor_id, a);
    }

    // ── Batch fetch ───────────────────────────────────────────────────────────

    #[test]
    fn batch_fetch_nodes() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let fake = NodeId(9999);

        let results = store.get_nodes_batch("test", &[a, fake, b]).unwrap();
        assert_eq!(results.len(), 3);
        assert!(results[0].is_some());
        assert!(results[1].is_none()); // fake id
        assert!(results[2].is_some());
    }

    #[test]
    fn batch_fetch_edges() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let eid = store
            .create_edge("test", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        let fake = EdgeId(9999);

        let results = store.get_edges_batch("test", &[eid, fake]).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].is_some());
        assert!(results[1].is_none());
    }

    // ── Delete with detach ────────────────────────────────────────────────────

    #[test]
    fn delete_node_without_detach_fails_if_has_edges() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        store
            .create_edge("test", a, b, "R".to_string(), HashMap::new())
            .unwrap();

        let result = store.delete_node("test", a, false);
        assert!(result.is_err(), "should refuse to delete connected node");
    }

    #[test]
    fn delete_node_with_detach_removes_edges() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let eid = store
            .create_edge("test", a, b, "R".to_string(), HashMap::new())
            .unwrap();

        store.delete_node("test", a, true).unwrap();
        assert!(store.get_node("test", a).unwrap().is_none());
        assert!(store.get_edge("test", eid).unwrap().is_none());
        // b still exists, its in_adj for the deleted edge is cleaned up
        assert!(store.in_edges("test", b).unwrap().is_empty());
    }

    #[test]
    fn detach_delete_cleans_both_directions() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        // edges in both directions
        let e1 = store
            .create_edge("test", a, b, "A_TO_B".to_string(), HashMap::new())
            .unwrap();
        let e2 = store
            .create_edge("test", b, a, "B_TO_A".to_string(), HashMap::new())
            .unwrap();

        store.delete_node("test", a, true).unwrap();

        assert!(store.get_edge("test", e1).unwrap().is_none());
        assert!(store.get_edge("test", e2).unwrap().is_none());
        assert!(store.out_edges("test", b).unwrap().is_empty());
        assert!(store.in_edges("test", b).unwrap().is_empty());
    }

    // ── Graph not found ───────────────────────────────────────────────────────

    #[test]
    fn graph_not_found_errors() {
        let store = InMemoryGraphStore::new();
        let result = store.get_node("no_such_graph", NodeId(1));
        assert!(matches!(result, Err(StorageError::GraphNotFound { .. })));

        let result = store.find_nodes_by_label("no_such_graph", "Person");
        assert!(matches!(result, Err(StorageError::GraphNotFound { .. })));
    }

    // ── Duplicate graph creation ───────────────────────────────────────────────

    #[test]
    fn duplicate_graph_creation_fails() {
        let store = InMemoryGraphStore::new();
        store.create_graph("mygraph").unwrap();
        let result = store.create_graph("mygraph");
        assert!(matches!(
            result,
            Err(StorageError::GraphAlreadyExists { .. })
        ));
    }

    // ── Invalid graph name ────────────────────────────────────────────────────

    #[test]
    fn invalid_graph_name_on_create() {
        let store = InMemoryGraphStore::new();
        assert!(store.create_graph("bad:name").is_err());
        assert!(store.create_graph("{slot}graph").is_err());
        assert!(store.create_graph("").is_err());
    }

    // ── Graph administration ──────────────────────────────────────────────────

    #[test]
    fn graph_exists_and_list() {
        let store = InMemoryGraphStore::new();
        assert!(!store.graph_exists("g1").unwrap());
        store.create_graph("g1").unwrap();
        store.create_graph("g2").unwrap();
        assert!(store.graph_exists("g1").unwrap());
        let mut names = store.list_graphs().unwrap();
        names.sort();
        assert_eq!(names, vec!["g1", "g2"]);
    }

    #[test]
    fn graph_stats_correct() {
        let store = make_store();
        let a = store
            .create_node("test", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("test", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("test", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        let stats = store.graph_stats("test").unwrap();
        assert_eq!(stats.node_count, 2);
        assert_eq!(stats.edge_count, 1);
        assert_eq!(stats.label_count, 1); // only "Person"
        assert_eq!(stats.rel_type_count, 1); // only "KNOWS"
    }

    // ── Node / edge property management ──────────────────────────────────────

    #[test]
    fn set_and_remove_node_properties() {
        let store = make_store();
        let id = store.create_node("test", vec![], HashMap::new()).unwrap();

        let mut props = HashMap::new();
        props.insert("x".to_string(), Value::Int(1));
        store.set_node_properties("test", id, props).unwrap();

        let node = store.get_node("test", id).unwrap().unwrap();
        assert_eq!(node.properties["x"], Value::Int(1));

        store.remove_node_property("test", id, "x").unwrap();
        let node = store.get_node("test", id).unwrap().unwrap();
        assert!(!node.properties.contains_key("x"));
    }

    #[test]
    fn add_and_remove_labels() {
        let store = make_store();
        let id = store
            .create_node("test", vec!["A".to_string()], HashMap::new())
            .unwrap();

        store
            .add_node_labels("test", id, &["B".to_string()])
            .unwrap();
        let node = store.get_node("test", id).unwrap().unwrap();
        assert!(node.labels.contains(&"B".to_string()));

        // Label index updated
        let b_nodes = store.find_nodes_by_label("test", "B").unwrap();
        assert!(b_nodes.contains(&id));

        store
            .remove_node_labels("test", id, &["A".to_string()])
            .unwrap();
        let node = store.get_node("test", id).unwrap().unwrap();
        assert!(!node.labels.contains(&"A".to_string()));
        let a_nodes = store.find_nodes_by_label("test", "A").unwrap();
        assert!(!a_nodes.contains(&id));
    }

    // ── Indexes ───────────────────────────────────────────────────────────────

    #[test]
    fn index_create_list_drop() {
        let store = make_store();
        store.create_index("test", "Person", "name").unwrap();
        store.create_index("test", "Person", "age").unwrap();

        let indexes = store.list_indexes("test").unwrap();
        assert_eq!(indexes.len(), 2);

        store.drop_index("test", "Person", "name").unwrap();
        let indexes = store.list_indexes("test").unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].property, "age");
    }

    #[test]
    fn create_index_idempotent() {
        let store = make_store();
        store.create_index("test", "Person", "name").unwrap();
        store.create_index("test", "Person", "name").unwrap(); // duplicate
        assert_eq!(store.list_indexes("test").unwrap().len(), 1);
    }

    // ── Edge endpoints must exist ─────────────────────────────────────────────

    #[test]
    fn create_edge_requires_existing_nodes() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let bad = NodeId(9999);
        let result = store.create_edge("test", a, bad, "R".to_string(), HashMap::new());
        assert!(matches!(result, Err(StorageError::NodeNotFound { .. })));
    }

    // ── all_node_ids ─────────────────────────────────────────────────────────

    #[test]
    fn all_node_ids_returns_all() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let mut ids = store.all_node_ids("test").unwrap();
        ids.sort();
        assert_eq!(ids, vec![a, b]);
    }

    // ── find_edges_by_type ────────────────────────────────────────────────────

    #[test]
    fn find_edges_by_type_returns_correct_edges() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let c = store.create_node("test", vec![], HashMap::new()).unwrap();
        let e1 = store
            .create_edge("test", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();
        let _e2 = store
            .create_edge("test", b, c, "LIKES".to_string(), HashMap::new())
            .unwrap();

        let knows = store.find_edges_by_type("test", "KNOWS").unwrap();
        assert_eq!(knows, vec![e1]);
    }

    // ── out_edge_ids / in_edge_ids ────────────────────────────────────────────

    #[test]
    fn out_edge_ids_and_in_edge_ids() {
        let store = make_store();
        let a = store.create_node("test", vec![], HashMap::new()).unwrap();
        let b = store.create_node("test", vec![], HashMap::new()).unwrap();
        let eid = store
            .create_edge("test", a, b, "R".to_string(), HashMap::new())
            .unwrap();

        let out_ids = store.out_edge_ids("test", a).unwrap();
        assert_eq!(out_ids, vec![eid]);
        let in_ids = store.in_edge_ids("test", b).unwrap();
        assert_eq!(in_ids, vec![eid]);
    }

    // ── Snapshot / merge ─────────────────────────────────────────────────────

    #[test]
    fn snapshot_creates_independent_copy() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let id = store
            .create_node("test", vec!["Person".to_string()], props)
            .unwrap();

        let snap = store.snapshot(&["test"]).unwrap();

        // Snapshot has the same node
        let node = snap.get_node("test", id).unwrap().unwrap();
        assert_eq!(node.properties["name"], Value::Str("Alice".into()));

        // Mutate the snapshot -- original is unaffected
        snap.create_node("test", vec!["Extra".to_string()], HashMap::new())
            .unwrap();
        let snap_stats = snap.graph_stats("test").unwrap();
        let orig_stats = store.graph_stats("test").unwrap();
        assert_eq!(snap_stats.node_count, 2);
        assert_eq!(orig_stats.node_count, 1);
    }

    #[test]
    fn snapshot_skips_missing_graphs() {
        let store = make_store();
        let snap = store.snapshot(&["test", "nonexistent"]).unwrap();
        assert!(snap.graph_exists("test").unwrap());
        assert!(!snap.graph_exists("nonexistent").unwrap());
    }

    #[test]
    fn merge_from_overwrites_existing_graph() {
        let store = make_store();
        store
            .create_node("test", vec!["Original".to_string()], HashMap::new())
            .unwrap();
        assert_eq!(store.graph_stats("test").unwrap().node_count, 1);

        let snap = store.snapshot(&["test"]).unwrap();
        snap.create_node("test", vec!["Added".to_string()], HashMap::new())
            .unwrap();
        assert_eq!(snap.graph_stats("test").unwrap().node_count, 2);

        store.merge_from(&snap).unwrap();
        assert_eq!(store.graph_stats("test").unwrap().node_count, 2);
    }

    #[test]
    fn merge_from_adds_new_graphs() {
        let store = make_store();
        let other = InMemoryGraphStore::new();
        other.create_graph("new_graph").unwrap();
        other
            .create_node("new_graph", vec![], HashMap::new())
            .unwrap();

        store.merge_from(&other).unwrap();
        assert!(store.graph_exists("new_graph").unwrap());
        assert_eq!(store.graph_stats("new_graph").unwrap().node_count, 1);
    }

    // ── BTreeMap property index tests ────────────────────────────────────────

    fn make_indexed_store() -> InMemoryGraphStore {
        let s = make_store();
        s.create_index("test", "Person", "age").unwrap();
        s.create_index("test", "Person", "name").unwrap();
        s
    }

    #[test]
    fn indexed_eq_query() {
        let store = make_indexed_store();
        let p1 = HashMap::from([("age".to_string(), Value::Int(30))]);
        let a = store
            .create_node("test", vec!["Person".to_string()], p1)
            .unwrap();
        let p2 = HashMap::from([("age".to_string(), Value::Int(25))]);
        let _b = store
            .create_node("test", vec!["Person".to_string()], p2)
            .unwrap();

        let bound = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert_eq!(result, vec![a]);
    }

    #[test]
    fn indexed_lt_lte_gt_gte_queries() {
        let store = make_indexed_store();
        let ages = vec![10, 20, 30, 40, 50];
        let mut ids = Vec::new();
        for age in &ages {
            let p = HashMap::from([("age".to_string(), Value::Int(*age))]);
            ids.push(
                store
                    .create_node("test", vec!["Person".to_string()], p)
                    .unwrap(),
            );
        }

        let bound = Value::Int(30);
        // Lt(30): ages 10, 20
        let mut lt = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Lt(&bound))
            .unwrap();
        lt.sort();
        assert_eq!(lt, vec![ids[0], ids[1]]);

        // Lte(30): ages 10, 20, 30
        let mut lte = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Lte(&bound))
            .unwrap();
        lte.sort();
        assert_eq!(lte, vec![ids[0], ids[1], ids[2]]);

        // Gt(30): ages 40, 50
        let mut gt = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Gt(&bound))
            .unwrap();
        gt.sort();
        assert_eq!(gt, vec![ids[3], ids[4]]);

        // Gte(30): ages 30, 40, 50
        let mut gte = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Gte(&bound))
            .unwrap();
        gte.sort();
        assert_eq!(gte, vec![ids[2], ids[3], ids[4]]);
    }

    #[test]
    fn indexed_between_query() {
        let store = make_indexed_store();
        let ages = vec![10, 20, 30, 40, 50];
        let mut ids = Vec::new();
        for age in &ages {
            let p = HashMap::from([("age".to_string(), Value::Int(*age))]);
            ids.push(
                store
                    .create_node("test", vec!["Person".to_string()], p)
                    .unwrap(),
            );
        }

        let lo = Value::Int(20);
        let hi = Value::Int(40);
        let mut result = store
            .find_nodes_by_label_and_property(
                "test",
                "Person",
                "age",
                RangeQuery::Between {
                    lo: &lo,
                    hi: &hi,
                    inclusive: true,
                },
            )
            .unwrap();
        result.sort();
        assert_eq!(result, vec![ids[1], ids[2], ids[3]]);

        // Non-inclusive hi
        let mut result = store
            .find_nodes_by_label_and_property(
                "test",
                "Person",
                "age",
                RangeQuery::Between {
                    lo: &lo,
                    hi: &hi,
                    inclusive: false,
                },
            )
            .unwrap();
        result.sort();
        assert_eq!(result, vec![ids[1], ids[2]]);
    }

    #[test]
    fn indexed_starts_with_query() {
        let store = make_indexed_store();
        let names = vec!["Alice", "Albert", "Bob", "Charlie"];
        let mut ids = Vec::new();
        for name in &names {
            let p = HashMap::from([("name".to_string(), Value::Str((*name).into()))]);
            ids.push(
                store
                    .create_node("test", vec!["Person".to_string()], p)
                    .unwrap(),
            );
        }

        let mut result = store
            .find_nodes_by_label_and_property(
                "test",
                "Person",
                "name",
                RangeQuery::StartsWith("Al"),
            )
            .unwrap();
        result.sort();
        assert_eq!(result, vec![ids[0], ids[1]]);
    }

    #[test]
    fn index_backfill_existing_nodes() {
        let store = make_store();
        // Create nodes BEFORE the index
        let p1 = HashMap::from([("age".to_string(), Value::Int(30))]);
        let a = store
            .create_node("test", vec!["Person".to_string()], p1)
            .unwrap();
        let p2 = HashMap::from([("age".to_string(), Value::Int(25))]);
        let _b = store
            .create_node("test", vec!["Person".to_string()], p2)
            .unwrap();

        // Create index AFTER nodes exist -- should backfill
        store.create_index("test", "Person", "age").unwrap();

        let bound = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert_eq!(result, vec![a]);
    }

    #[test]
    fn index_maintained_on_property_update() {
        let store = make_indexed_store();
        let p = HashMap::from([("age".to_string(), Value::Int(30))]);
        let a = store
            .create_node("test", vec!["Person".to_string()], p)
            .unwrap();

        // Update property
        store
            .set_node_properties(
                "test",
                a,
                HashMap::from([("age".to_string(), Value::Int(40))]),
            )
            .unwrap();

        // Old value should not match
        let bound30 = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound30))
            .unwrap();
        assert!(result.is_empty());

        // New value should match
        let bound40 = Value::Int(40);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound40))
            .unwrap();
        assert_eq!(result, vec![a]);
    }

    #[test]
    fn index_maintained_on_property_replace() {
        let store = make_indexed_store();
        let p = HashMap::from([("age".to_string(), Value::Int(30))]);
        let a = store
            .create_node("test", vec!["Person".to_string()], p)
            .unwrap();

        store
            .replace_node_properties(
                "test",
                a,
                HashMap::from([("age".to_string(), Value::Int(50))]),
            )
            .unwrap();

        let bound30 = Value::Int(30);
        assert!(store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound30))
            .unwrap()
            .is_empty());

        let bound50 = Value::Int(50);
        assert_eq!(
            store
                .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound50))
                .unwrap(),
            vec![a]
        );
    }

    #[test]
    fn index_maintained_on_property_remove() {
        let store = make_indexed_store();
        let p = HashMap::from([("age".to_string(), Value::Int(30))]);
        let a = store
            .create_node("test", vec!["Person".to_string()], p)
            .unwrap();

        store.remove_node_property("test", a, "age").unwrap();

        let bound = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn index_maintained_on_node_delete() {
        let store = make_indexed_store();
        let p = HashMap::from([("age".to_string(), Value::Int(30))]);
        let a = store
            .create_node("test", vec!["Person".to_string()], p)
            .unwrap();

        store.delete_node("test", a, false).unwrap();

        let bound = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn drop_index_cleans_btreemap() {
        let store = make_indexed_store();
        let p = HashMap::from([("age".to_string(), Value::Int(30))]);
        store
            .create_node("test", vec!["Person".to_string()], p)
            .unwrap();

        store.drop_index("test", "Person", "age").unwrap();

        // After dropping, lookups still work (fall back to full scan)
        let bound = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert_eq!(result.len(), 1);
    }

    // ── Snapshot isolation ──────────────────────────────────────────────────

    #[test]
    fn snapshot_isolation_readers_see_pre_mutation_state() {
        use std::sync::Arc as StdArc;

        let store = StdArc::new(make_store());
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let id = store
            .create_node("test", vec!["Person".to_string()], props)
            .unwrap();

        // Reader takes a snapshot *before* the writer mutates.
        let snap = store.snapshot_handle().unwrap();

        // Verify the snapshot sees the original state.
        let g = snap.get_graph("test").unwrap();
        assert_eq!(g.nodes.len(), 1);
        let node = g.nodes.get(&id.0).unwrap();
        assert_eq!(node.properties["name"], Value::Str("Alice".into()));

        // Writer mutates the store concurrently.
        let mut new_props = HashMap::new();
        new_props.insert("name".to_string(), Value::Str("Bob".into()));
        store.set_node_properties("test", id, new_props).unwrap();

        // Also add a new node.
        store
            .create_node("test", vec!["Person".to_string()], HashMap::new())
            .unwrap();

        // The snapshot STILL sees the old state (1 node, name=Alice).
        let g = snap.get_graph("test").unwrap();
        assert_eq!(g.nodes.len(), 1, "snapshot must not see the new node");
        let node = g.nodes.get(&id.0).unwrap();
        assert_eq!(
            node.properties["name"],
            Value::Str("Alice".into()),
            "snapshot must see pre-mutation name"
        );

        // But a fresh read from the store sees the updated state.
        let fresh = store.get_node("test", id).unwrap().unwrap();
        assert_eq!(fresh.properties["name"], Value::Str("Bob".into()));
        assert_eq!(store.graph_stats("test").unwrap().node_count, 2);
    }

    #[test]
    fn snapshot_isolation_concurrent_threads() {
        use std::sync::Arc as StdArc;
        use std::sync::Barrier;

        let store = StdArc::new(make_store());
        let mut props = HashMap::new();
        props.insert("v".to_string(), Value::Int(1));
        let id = store
            .create_node("test", vec!["N".to_string()], props)
            .unwrap();

        let barrier = StdArc::new(Barrier::new(2));

        // Reader thread: takes snapshot, waits for writer, then asserts old value.
        let store_r = StdArc::clone(&store);
        let barrier_r = StdArc::clone(&barrier);
        let reader = std::thread::spawn(move || {
            let snap = store_r.snapshot_handle().unwrap();
            // Signal writer to proceed.
            barrier_r.wait();
            // Wait for writer to finish.
            barrier_r.wait();
            // Snapshot must still show old value.
            let g = snap.get_graph("test").unwrap();
            let node = g.nodes.get(&id.0).unwrap();
            assert_eq!(
                node.properties["v"],
                Value::Int(1),
                "reader snapshot must be isolated from concurrent write"
            );
        });

        // Writer thread: waits for reader to snapshot, then mutates.
        let store_w = StdArc::clone(&store);
        let barrier_w = StdArc::clone(&barrier);
        let writer = std::thread::spawn(move || {
            // Wait for reader to take snapshot.
            barrier_w.wait();
            let mut new_props = HashMap::new();
            new_props.insert("v".to_string(), Value::Int(999));
            store_w.set_node_properties("test", id, new_props).unwrap();
            // Signal reader that write is done.
            barrier_w.wait();
        });

        reader.join().unwrap();
        writer.join().unwrap();

        // Post-test: store reflects the write.
        let node = store.get_node("test", id).unwrap().unwrap();
        assert_eq!(node.properties["v"], Value::Int(999));
    }
}
