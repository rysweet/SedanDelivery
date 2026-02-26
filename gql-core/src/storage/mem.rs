//! In-memory graph store — primary implementation for unit tests, CLI, and Phase 0.
//!
//! All data lives in a single `RwLock<Inner>`. Reads acquire a shared lock; writes acquire
//! an exclusive lock. There are no sub-graph locks; the store is optimised for correctness
//! and test velocity, not raw throughput (the Redis backend handles production load).

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use crate::error::StorageError;
use crate::storage::{
    validate_graph_name, AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery,
};
use crate::types::Value;

// ── Per-graph state ────────────────────────────────────────────────────────────

/// All mutable state for a single named graph.
struct GraphInner {
    nodes: HashMap<u64, NodeData>,
    edges: HashMap<u64, EdgeData>,
    /// Outgoing adjacency: node_id -> list of (edge_id, neighbor_id, rel_type)
    out_adj: HashMap<u64, Vec<AdjEntry>>,
    /// Incoming adjacency: node_id -> list of (edge_id, neighbor_id, rel_type)
    in_adj: HashMap<u64, Vec<AdjEntry>>,
    /// label -> set of node IDs
    label_index: HashMap<String, HashSet<u64>>,
    /// rel_type -> set of edge IDs
    reltype_index: HashMap<String, HashSet<u64>>,
    indexes: Vec<IndexDescriptor>,
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
/// All operations are `O(1)` amortised for single-item reads/writes. Batch reads
/// are a single pass over the internal `HashMap`. Label scans are `O(|label set|)`.
pub struct InMemoryGraphStore {
    /// map: graph_name -> per-graph state
    inner: RwLock<HashMap<String, GraphInner>>,
}

impl InMemoryGraphStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    /// Acquire a read lock, mapping the `PoisonError` to `StorageError::Backend`.
    fn read(
        &self,
    ) -> Result<std::sync::RwLockReadGuard<'_, HashMap<String, GraphInner>>, StorageError> {
        self.inner
            .read()
            .map_err(|e| StorageError::backend(format!("lock poisoned (read): {e}")))
    }

    /// Acquire a write lock.
    fn write(
        &self,
    ) -> Result<std::sync::RwLockWriteGuard<'_, HashMap<String, GraphInner>>, StorageError> {
        self.inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned (write): {e}")))
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

// ── GraphStore impl ───────────────────────────────────────────────────────────

impl GraphStore for InMemoryGraphStore {
    // ── Single-item reads ──────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
        Ok(g.nodes.get(&id.0).cloned())
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
        Ok(g.edges.get(&id.0).cloned())
    }

    // ── Batch reads ────────────────────────────────────────────────────────────

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
        Ok(ids.iter().map(|id| g.nodes.get(&id.0).cloned()).collect())
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
        Ok(ids.iter().map(|id| g.edges.get(&id.0).cloned()).collect())
    }

    // ── Label/type index reads ─────────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
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
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
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
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
        let ids = g
            .reltype_index
            .get(rel_type)
            .map(|s| s.iter().copied().map(EdgeId).collect())
            .unwrap_or_default();
        Ok(ids)
    }

    // ── Adjacency reads ────────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
        Ok(g.out_adj.get(&node_id.0).cloned().unwrap_or_default())
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
        Ok(g.in_adj.get(&node_id.0).cloned().unwrap_or_default())
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
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
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
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
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
        Ok(g.out_adj
            .get(&node_id.0)
            .map(|v| v.iter().map(|e| e.edge_id).collect())
            .unwrap_or_default())
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
        Ok(g.in_adj
            .get(&node_id.0)
            .map(|v| v.iter().map(|e| e.edge_id).collect())
            .unwrap_or_default())
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
        Ok(g.nodes.keys().copied().map(NodeId).collect())
    }

    // ── Write operations ───────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: HashMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        let mut guard = self.write()?;
        let g = Self::require_graph_mut(&mut guard, graph)?;
        let raw_id = g.next_node_id(graph)?;
        let node_id = NodeId(raw_id);
        let node = NodeData {
            id: node_id,
            labels: labels.clone(),
            properties,
        };
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
        let g = Self::require_graph_mut(&mut guard, graph)?;
        // Validate that both endpoints exist.
        if !g.nodes.contains_key(&src.0) {
            return Err(StorageError::NodeNotFound { id: src.0 });
        }
        if !g.nodes.contains_key(&dst.0) {
            return Err(StorageError::NodeNotFound { id: dst.0 });
        }
        let raw_id = g.next_edge_id(graph)?;
        let edge_id = EdgeId(raw_id);
        let edge = EdgeData {
            id: edge_id,
            rel_type: rel_type.clone(),
            src,
            dst,
            properties,
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
        let g = Self::require_graph_mut(&mut guard, graph)?;
        let node = g
            .nodes
            .get_mut(&id.0)
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        // Merge: overwrite keys present in `properties`, leave others alone.
        for (k, v) in properties {
            node.properties.insert(k, v);
        }
        Ok(())
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let g = Self::require_graph_mut(&mut guard, graph)?;
        let edge = g
            .edges
            .get_mut(&id.0)
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        for (k, v) in properties {
            edge.properties.insert(k, v);
        }
        Ok(())
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let g = Self::require_graph_mut(&mut guard, graph)?;
        let node = g
            .nodes
            .get_mut(&id.0)
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        node.properties.remove(key);
        Ok(())
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let g = Self::require_graph_mut(&mut guard, graph)?;
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
        let g = Self::require_graph_mut(&mut guard, graph)?;
        let node = g
            .nodes
            .get_mut(&id.0)
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        for label in labels {
            if !node.labels.contains(label) {
                node.labels.push(label.clone());
            }
        }
        // Update label index after borrowing node immutably is done.
        // We need to re-borrow. Collect labels to add first to avoid borrow conflict.
        let to_add: Vec<String> = labels.to_vec();
        for label in to_add {
            g.label_index.entry(label).or_default().insert(id.0);
        }
        Ok(())
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let g = Self::require_graph_mut(&mut guard, graph)?;
        let node = g
            .nodes
            .get_mut(&id.0)
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        for label in labels {
            node.labels.retain(|l| l != label);
        }
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
        let g = Self::require_graph_mut(&mut guard, graph)?;

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
            // Collect all edge IDs attached to this node.
            let out_edges: Vec<(u64, NodeId)> = g
                .out_adj
                .get(&id.0)
                .map(|v| v.iter().map(|e| (e.edge_id.0, e.neighbor_id)).collect())
                .unwrap_or_default();
            let in_edges: Vec<(u64, NodeId)> = g
                .in_adj
                .get(&id.0)
                .map(|v| v.iter().map(|e| (e.edge_id.0, e.neighbor_id)).collect())
                .unwrap_or_default();

            // Remove each outgoing edge.
            for (eid, dst) in &out_edges {
                // Remove from reltype index.
                if let Some(edge) = g.edges.remove(eid) {
                    if let Some(set) = g.reltype_index.get_mut(&edge.rel_type) {
                        set.remove(eid);
                        if set.is_empty() {
                            g.reltype_index.remove(&edge.rel_type);
                        }
                    }
                }
                // Remove from dst's in_adj.
                if let Some(v) = g.in_adj.get_mut(&dst.0) {
                    v.retain(|e| e.edge_id.0 != *eid);
                }
            }

            // Remove each incoming edge.
            for (eid, src) in &in_edges {
                if let Some(edge) = g.edges.remove(eid) {
                    if let Some(set) = g.reltype_index.get_mut(&edge.rel_type) {
                        set.remove(eid);
                        if set.is_empty() {
                            g.reltype_index.remove(&edge.rel_type);
                        }
                    }
                }
                // Remove from src's out_adj.
                if let Some(v) = g.out_adj.get_mut(&src.0) {
                    v.retain(|e| e.edge_id.0 != *eid);
                }
            }

            g.out_adj.remove(&id.0);
            g.in_adj.remove(&id.0);
        }

        // Remove from label index.
        if let Some(node) = g.nodes.remove(&id.0) {
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
        let g = Self::require_graph_mut(&mut guard, graph)?;

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
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
        Ok(g.indexes.clone())
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let g = Self::require_graph_mut(&mut guard, graph)?;
        let desc = IndexDescriptor {
            label: label.to_string(),
            property: property.to_string(),
        };
        // Idempotent: only add if not already present.
        if !g
            .indexes
            .iter()
            .any(|i| i.label == label && i.property == property)
        {
            g.indexes.push(desc);
        }
        Ok(())
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        let g = Self::require_graph_mut(&mut guard, graph)?;
        g.indexes
            .retain(|i| !(i.label == label && i.property == property));
        Ok(())
    }

    // ── Graph administration ───────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        let mut guard = self.write()?;
        if guard.contains_key(name) {
            return Err(StorageError::GraphAlreadyExists {
                name: name.to_string(),
            });
        }
        guard.insert(name.to_string(), GraphInner::new());
        Ok(())
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        let mut guard = self.write()?;
        if guard.remove(name).is_none() {
            return Err(StorageError::GraphNotFound {
                name: name.to_string(),
            });
        }
        Ok(())
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        let guard = self.read()?;
        Ok(guard.contains_key(name))
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        let guard = self.read()?;
        let mut names: Vec<String> = guard.keys().cloned().collect();
        names.sort();
        Ok(names)
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        let guard = self.read()?;
        let g = Self::require_graph(&guard, graph)?;
        Ok(GraphStats {
            node_count: g.nodes.len() as u64,
            edge_count: g.edges.len() as u64,
            label_count: g.label_index.len() as u32,
            rel_type_count: g.reltype_index.len() as u32,
        })
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
}
