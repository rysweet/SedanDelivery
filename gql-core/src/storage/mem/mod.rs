//! In-memory graph store — primary implementation for unit tests, CLI, and Phase 0.
//!
//! All data lives in a single `RwLock<Inner>`. Reads acquire a shared lock; writes acquire
//! an exclusive lock. There are no sub-graph locks; the store is optimised for correctness
//! and test velocity, not raw throughput (the Redis backend handles production load).
//!
//! # Module layout
//!
//! The implementation is split across sub-modules for maintainability:
//! - [`nodes`] — Node CRUD (create, get, delete, properties, labels)
//! - [`edges`] — Edge CRUD (create, get, delete, properties)
//! - [`indexing`] — Property index management (create, drop, list, BTreeMap helpers)
//! - [`query`] — Read-only query helpers (range matching for fallback scans)

mod edges;
mod indexing;
mod nodes;
mod query;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, RwLock};

use crate::error::StorageError;
use crate::storage::{
    validate_graph_name, AdjEntry, ConstraintDescriptor, ConstraintType, EdgeData, EdgeId,
    GraphStats, GraphStore, IndexDescriptor, NodeData, NodeId, RangeQuery,
};
use crate::types::Value;

// ── IndexableValue: Ord-compatible wrapper for BTreeMap keys ──────────────────

/// A value that can be used as a BTreeMap key for property indexes.
/// Wraps the subset of `Value` variants that support total ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IndexableValue {
    Bool(bool),
    Int(i64),
    /// f64 stored as raw bits for Ord. NaN values are excluded at insertion time.
    Float(u64),
    Str(Box<str>),
}

impl IndexableValue {
    /// Convert a `Value` to an `IndexableValue`, returning `None` for non-indexable types.
    pub(crate) fn from_value(v: &Value) -> Option<Self> {
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
pub(crate) type PropIndexKey = (String, String, IndexableValue);

/// The property index: maps (label, property, value) -> set of raw node IDs.
pub(crate) type PropIndex = BTreeMap<PropIndexKey, HashSet<u64>>;

// ── Per-graph state ────────────────────────────────────────────────────────────

/// All mutable state for a single named graph.
#[derive(Clone)]
pub(crate) struct GraphInner {
    pub(crate) nodes: HashMap<u64, NodeData>,
    pub(crate) edges: HashMap<u64, EdgeData>,
    pub(crate) out_adj: HashMap<u64, Vec<AdjEntry>>,
    pub(crate) in_adj: HashMap<u64, Vec<AdjEntry>>,
    pub(crate) label_index: HashMap<String, HashSet<u64>>,
    pub(crate) reltype_index: HashMap<String, HashSet<u64>>,
    pub(crate) indexes: Vec<IndexDescriptor>,
    pub(crate) prop_index: PropIndex,
    pub(crate) constraints: HashSet<ConstraintDescriptor>,
    pub(crate) node_seq: u64,
    pub(crate) edge_seq: u64,
}

impl GraphInner {
    pub(crate) fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
            out_adj: HashMap::new(),
            in_adj: HashMap::new(),
            label_index: HashMap::new(),
            reltype_index: HashMap::new(),
            indexes: Vec::new(),
            prop_index: BTreeMap::new(),
            constraints: HashSet::new(),
            node_seq: 0,
            edge_seq: 0,
        }
    }

    pub(crate) fn next_node_id(&mut self, graph_name: &str) -> Result<u64, StorageError> {
        self.node_seq = self
            .node_seq
            .checked_add(1)
            .ok_or_else(|| StorageError::IdOverflow {
                graph: graph_name.to_string(),
            })?;
        Ok(self.node_seq)
    }

    pub(crate) fn has_index(&self, label: &str, property: &str) -> bool {
        self.indexes
            .iter()
            .any(|i| i.label == label && i.property == property)
    }

    pub(crate) fn index_insert(
        &mut self,
        raw_id: u64,
        labels: &[String],
        props: &HashMap<String, Value>,
    ) {
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

    pub(crate) fn index_remove(
        &mut self,
        raw_id: u64,
        labels: &[String],
        props: &HashMap<String, Value>,
    ) {
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

    /// Check uniqueness constraints for a node being created or updated.
    /// `exclude_node_id` is the node's own ID (to exclude self when checking
    /// updates); pass `None` for new node creation.
    pub(crate) fn check_uniqueness_constraints(
        &self,
        labels: &[String],
        properties: &HashMap<String, Value>,
        exclude_node_id: Option<u64>,
    ) -> Result<(), StorageError> {
        for constraint in &self.constraints {
            if constraint.constraint_type != ConstraintType::Unique {
                continue;
            }
            if !labels.contains(&constraint.label) {
                continue;
            }
            let prop_val = match properties.get(&constraint.property) {
                Some(v) => v,
                None => continue,
            };
            // Check all existing nodes with the same label for a duplicate value.
            if let Some(label_nodes) = self.label_index.get(&constraint.label) {
                for &raw_id in label_nodes {
                    if exclude_node_id == Some(raw_id) {
                        continue;
                    }
                    if let Some(node) = self.nodes.get(&raw_id) {
                        if let Some(existing_val) = node.properties.get(&constraint.property) {
                            if existing_val == prop_val {
                                return Err(StorageError::ConstraintViolation {
                                    msg: format!(
                                        "Node(:{}) already exists with property `{}` = {:?}",
                                        constraint.label, constraint.property, prop_val
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn next_edge_id(&mut self, graph_name: &str) -> Result<u64, StorageError> {
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
    pub(crate) inner: RwLock<Arc<HashMap<String, GraphInner>>>,
}

impl std::fmt::Debug for InMemoryGraphStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.inner.read() {
            Ok(g) => {
                let graph_names: Vec<String> = g.keys().cloned().collect();
                f.debug_struct("InMemoryGraphStore")
                    .field("graphs", &graph_names)
                    .finish()
            }
            Err(_) => f.write_str("[lock poisoned]"),
        }
    }
}

impl InMemoryGraphStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Arc::new(HashMap::new())),
        }
    }

    pub(crate) fn read_snapshot(&self) -> Result<Arc<HashMap<String, GraphInner>>, StorageError> {
        let guard = self
            .inner
            .read()
            .map_err(|e| StorageError::backend(format!("lock poisoned (read): {e}")))?;
        Ok(Arc::clone(&guard))
    }

    pub(crate) fn write(
        &self,
    ) -> Result<std::sync::RwLockWriteGuard<'_, Arc<HashMap<String, GraphInner>>>, StorageError>
    {
        self.inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned (write): {e}")))
    }

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

    pub fn merge_from(&self, other: &InMemoryGraphStore) -> Result<(), StorageError> {
        let other_snap = other.read_snapshot()?;
        let mut self_guard = self.write()?;
        let map = Arc::make_mut(&mut self_guard);
        for (name, graph) in other_snap.iter() {
            map.insert(name.clone(), graph.clone());
        }
        Ok(())
    }

    pub(crate) fn require_graph<'g>(
        map: &'g HashMap<String, GraphInner>,
        graph: &str,
    ) -> Result<&'g GraphInner, StorageError> {
        map.get(graph).ok_or_else(|| StorageError::GraphNotFound {
            name: graph.to_string(),
        })
    }

    pub(crate) fn require_graph_mut<'g>(
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

// ── GraphStore impl ───────────────────────────────────────────────────────────

impl GraphStore for InMemoryGraphStore {
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

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        let mut ids: Vec<NodeId> = g
            .label_index
            .get(label)
            .map(|s| s.iter().copied().map(NodeId).collect())
            .unwrap_or_default();
        ids.sort_by_key(|id| id.0);
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

        if g.has_index(label, property) {
            return Ok(indexing::indexed_range_query(
                &g.prop_index,
                label,
                property,
                &range,
            ));
        }

        let candidate_ids = match g.label_index.get(label) {
            Some(s) => s,
            None => return Ok(vec![]),
        };
        let mut result = Vec::new();
        for &raw_id in candidate_ids {
            if let Some(node) = g.nodes.get(&raw_id) {
                if let Some(prop_val) = node.properties.get(property) {
                    if query::matches_range(prop_val, &range) {
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
        let mut ids: Vec<NodeId> = g.nodes.keys().copied().map(NodeId).collect();
        ids.sort_by_key(|id| id.0);
        Ok(ids)
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(g.edges.keys().copied().map(EdgeId).collect())
    }

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: HashMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        nodes::create_node_impl(self, graph, labels, properties)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: HashMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        edges::create_edge_impl(self, graph, src, dst, rel_type, properties)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        nodes::set_node_properties_impl(self, graph, id, properties)
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        nodes::replace_node_properties_impl(self, graph, id, properties)
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        edges::set_edge_properties_impl(self, graph, id, properties)
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        edges::replace_edge_properties_impl(self, graph, id, properties)
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        nodes::remove_node_property_impl(self, graph, id, key)
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        edges::remove_edge_property_impl(self, graph, id, key)
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        nodes::add_node_labels_impl(self, graph, id, labels)
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        nodes::remove_node_labels_impl(self, graph, id, labels)
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        nodes::delete_node_impl(self, graph, id, detach)
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        edges::delete_edge_impl(self, graph, id)
    }

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        indexing::list_indexes_impl(self, graph)
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        indexing::create_index_impl(self, graph, label, property)
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        indexing::drop_index_impl(self, graph, label, property)
    }

    fn list_constraints(&self, graph: &str) -> Result<Vec<ConstraintDescriptor>, StorageError> {
        let snap = self.read_snapshot()?;
        let g = Self::require_graph(&snap, graph)?;
        Ok(g.constraints.iter().cloned().collect())
    }

    fn create_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        constraint_type: ConstraintType,
    ) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = InMemoryGraphStore::require_graph_mut(map, graph)?;

        let descriptor = ConstraintDescriptor {
            label: label.to_string(),
            property: property.to_string(),
            constraint_type: constraint_type.clone(),
        };

        if g.constraints.contains(&descriptor) {
            return Err(StorageError::Backend {
                msg: format!(
                    "Constraint already exists for :{}.{} ({})",
                    label, property, constraint_type
                ),
            });
        }

        // Validate existing data does not violate the constraint.
        if constraint_type == ConstraintType::Unique {
            if let Some(label_nodes) = g.label_index.get(label) {
                // Collect (value_debug_string, raw_id) pairs for duplicate detection.
                // We use the Debug representation as a surrogate key since Value
                // does not implement Hash/Eq.
                let mut seen: HashMap<String, u64> = HashMap::new();
                for &raw_id in label_nodes {
                    if let Some(node) = g.nodes.get(&raw_id) {
                        if let Some(val) = node.properties.get(property) {
                            let key = format!("{:?}", val);
                            if let Some(&first_id) = seen.get(&key) {
                                return Err(StorageError::ConstraintViolation {
                                    msg: format!(
                                        "Cannot create UNIQUE constraint on :{}.{} — \
                                         nodes #{} and #{} both have value {:?}",
                                        label, property, first_id, raw_id, val
                                    ),
                                });
                            }
                            seen.insert(key, raw_id);
                        }
                    }
                }
            }
        }

        g.constraints.insert(descriptor);
        Ok(())
    }

    fn drop_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = InMemoryGraphStore::require_graph_mut(map, graph)?;

        // Remove any constraint matching label+property (regardless of type).
        let before = g.constraints.len();
        g.constraints
            .retain(|c| !(c.label == label && c.property == property));
        if g.constraints.len() == before {
            return Err(StorageError::Backend {
                msg: format!("No constraint found for :{}.{}", label, property),
            });
        }
        Ok(())
    }

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
pub(crate) mod test_helpers {
    use super::*;

    pub(crate) fn make_store() -> InMemoryGraphStore {
        let s = InMemoryGraphStore::new();
        s.create_graph("test").unwrap();
        s
    }

    pub(crate) fn make_indexed_store() -> InMemoryGraphStore {
        let s = make_store();
        s.create_index("test", "Person", "age").unwrap();
        s.create_index("test", "Person", "name").unwrap();
        s
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::make_store;
    use super::*;

    #[test]
    fn graph_not_found_errors() {
        let store = InMemoryGraphStore::new();
        let result = store.get_node("no_such_graph", NodeId(1));
        assert!(matches!(result, Err(StorageError::GraphNotFound { .. })));

        let result = store.find_nodes_by_label("no_such_graph", "Person");
        assert!(matches!(result, Err(StorageError::GraphNotFound { .. })));
    }

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

    #[test]
    fn invalid_graph_name_on_create() {
        let store = InMemoryGraphStore::new();
        assert!(store.create_graph("bad:name").is_err());
        assert!(store.create_graph("{slot}graph").is_err());
        assert!(store.create_graph("").is_err());
    }

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
        assert_eq!(stats.label_count, 1);
        assert_eq!(stats.rel_type_count, 1);
    }

    #[test]
    fn snapshot_creates_independent_copy() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let id = store
            .create_node("test", vec!["Person".to_string()], props)
            .unwrap();

        let snap = store.snapshot(&["test"]).unwrap();

        let node = snap.get_node("test", id).unwrap().unwrap();
        assert_eq!(node.properties["name"], Value::Str("Alice".into()));

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

    // ── Constraint tests ─────────────────────────────────────────────────

    #[test]
    fn create_uniqueness_constraint() {
        let store = make_store();
        store
            .create_constraint("test", "Person", "email", ConstraintType::Unique)
            .unwrap();
        let constraints = store.list_constraints("test").unwrap();
        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].label, "Person");
        assert_eq!(constraints[0].property, "email");
    }

    #[test]
    fn constraint_blocks_duplicate_on_create() {
        let store = make_store();
        store
            .create_constraint("test", "Person", "email", ConstraintType::Unique)
            .unwrap();

        let mut props = HashMap::new();
        props.insert("email".to_string(), Value::Str("alice@example.com".into()));
        store
            .create_node("test", vec!["Person".to_string()], props.clone())
            .unwrap();

        // Second node with the same email should fail.
        let result = store.create_node("test", vec!["Person".to_string()], props);
        assert!(
            matches!(result, Err(StorageError::ConstraintViolation { .. })),
            "Expected ConstraintViolation, got {:?}",
            result
        );
    }

    #[test]
    fn constraint_allows_different_values() {
        let store = make_store();
        store
            .create_constraint("test", "Person", "email", ConstraintType::Unique)
            .unwrap();

        let mut props1 = HashMap::new();
        props1.insert("email".to_string(), Value::Str("alice@example.com".into()));
        store
            .create_node("test", vec!["Person".to_string()], props1)
            .unwrap();

        let mut props2 = HashMap::new();
        props2.insert("email".to_string(), Value::Str("bob@example.com".into()));
        store
            .create_node("test", vec!["Person".to_string()], props2)
            .unwrap();
    }

    #[test]
    fn constraint_blocks_duplicate_on_set_properties() {
        let store = make_store();
        store
            .create_constraint("test", "Person", "email", ConstraintType::Unique)
            .unwrap();

        let mut props1 = HashMap::new();
        props1.insert("email".to_string(), Value::Str("alice@example.com".into()));
        store
            .create_node("test", vec!["Person".to_string()], props1)
            .unwrap();

        let mut props2 = HashMap::new();
        props2.insert("email".to_string(), Value::Str("bob@example.com".into()));
        let id2 = store
            .create_node("test", vec!["Person".to_string()], props2)
            .unwrap();

        // Try to update id2's email to alice's email.
        let mut update = HashMap::new();
        update.insert("email".to_string(), Value::Str("alice@example.com".into()));
        let result = store.set_node_properties("test", id2, update);
        assert!(matches!(
            result,
            Err(StorageError::ConstraintViolation { .. })
        ));
    }

    #[test]
    fn constraint_creation_fails_with_existing_duplicates() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("email".to_string(), Value::Str("alice@example.com".into()));
        store
            .create_node("test", vec!["Person".to_string()], props.clone())
            .unwrap();
        store
            .create_node("test", vec!["Person".to_string()], props)
            .unwrap();

        let result = store.create_constraint("test", "Person", "email", ConstraintType::Unique);
        assert!(matches!(
            result,
            Err(StorageError::ConstraintViolation { .. })
        ));
    }

    #[test]
    fn drop_constraint_removes_it() {
        let store = make_store();
        store
            .create_constraint("test", "Person", "email", ConstraintType::Unique)
            .unwrap();
        assert_eq!(store.list_constraints("test").unwrap().len(), 1);

        store.drop_constraint("test", "Person", "email").unwrap();
        assert_eq!(store.list_constraints("test").unwrap().len(), 0);
    }

    #[test]
    fn drop_constraint_allows_duplicate_after() {
        let store = make_store();
        store
            .create_constraint("test", "Person", "email", ConstraintType::Unique)
            .unwrap();

        let mut props = HashMap::new();
        props.insert("email".to_string(), Value::Str("alice@example.com".into()));
        store
            .create_node("test", vec!["Person".to_string()], props.clone())
            .unwrap();

        store.drop_constraint("test", "Person", "email").unwrap();

        // Now duplicate should succeed.
        store
            .create_node("test", vec!["Person".to_string()], props)
            .unwrap();
    }

    #[test]
    fn constraint_does_not_apply_to_different_labels() {
        let store = make_store();
        store
            .create_constraint("test", "Person", "email", ConstraintType::Unique)
            .unwrap();

        let mut props = HashMap::new();
        props.insert("email".to_string(), Value::Str("alice@example.com".into()));
        store
            .create_node("test", vec!["Person".to_string()], props.clone())
            .unwrap();

        // Creating a Company node with the same email should succeed.
        store
            .create_node("test", vec!["Company".to_string()], props)
            .unwrap();
    }
}
