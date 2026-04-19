//! In-memory graph store — primary implementation for unit tests, CLI, and Phase 0.
//!
//! All data lives in a single `RwLock<Inner>`. Reads acquire a shared lock; writes acquire
//! an exclusive lock. There are no sub-graph locks; the store is optimised for correctness
//! and test velocity, not raw throughput (the Redis backend handles production load).
//!
//! # Module layout
//!
//! The implementation is split across sub-modules for maintainability:
//! - [`indexable`] — `IndexableValue` Ord wrapper and `PropIndex` type aliases
//! - [`graph_inner`] — `GraphInner` per-graph state and index/constraint helpers
//! - [`store`] — `InMemoryGraphStore` struct, snapshot/merge, lock helpers
//! - [`nodes`] — Node CRUD (create, get, delete, properties, labels)
//! - [`edges`] — Edge CRUD (create, get, delete, properties)
//! - [`indexing`] — Property index management (create, drop, list, BTreeMap helpers)
//! - [`constraints`] — Uniqueness constraint create/drop/list helpers
//! - [`query`] — Read-only query helpers (range matching for fallback scans)

mod constraints;
mod edges;
mod graph_inner;
mod indexable;
mod indexing;
mod lifecycle;
mod nodes;
mod query;
mod store;

use indexmap::IndexMap;

use crate::error::StorageError;
use crate::storage::{
    AdjEntry, ConstraintDescriptor, ConstraintType, EdgeData, EdgeId, GraphStats, GraphStore,
    IndexDescriptor, NodeData, NodeId, RangeQuery,
};
use crate::types::Value;

pub(crate) use graph_inner::GraphInner;
pub(crate) use indexable::{IndexableValue, PropIndex, PropIndexKey};
pub use store::InMemoryGraphStore;

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

        // No declared index — do in-memory label scan + property filter.
        let label_nodes = self.find_nodes_by_label(graph, label)?;
        let mut matched = Vec::new();
        for id in label_nodes {
            if let Some(node) = self.get_node(graph, id)? {
                if let Some(val) = node.properties.get(property) {
                    let is_match = match &range {
                        RangeQuery::Eq(expected) => val == *expected,
                        _ => true,
                    };
                    if is_match {
                        matched.push(id);
                    }
                }
            }
        }

        Ok(matched)
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
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        nodes::create_node_impl(self, graph, labels, properties)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        edges::create_edge_impl(self, graph, src, dst, rel_type, properties)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        nodes::set_node_properties_impl(self, graph, id, properties)
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        nodes::replace_node_properties_impl(self, graph, id, properties)
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        edges::set_edge_properties_impl(self, graph, id, properties)
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
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
        constraints::list_constraints_impl(self, graph)
    }

    fn create_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        constraint_type: ConstraintType,
    ) -> Result<(), StorageError> {
        constraints::create_constraint_impl(self, graph, label, property, constraint_type)
    }

    fn drop_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        constraints::drop_constraint_impl(self, graph, label, property)
    }

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        lifecycle::create_graph_impl(self, name)
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        lifecycle::delete_graph_impl(self, name)
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        lifecycle::graph_exists_impl(self, name)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        lifecycle::list_graphs_impl(self)
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        lifecycle::graph_stats_impl(self, graph)
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        lifecycle::all_label_names_impl(self, graph)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn tx_snapshot(&self, graphs: &[&str]) -> Result<Option<Box<dyn GraphStore>>, StorageError> {
        lifecycle::tx_snapshot_impl(self, graphs)
    }

    fn tx_commit(&self, snapshot: &dyn GraphStore) -> Result<(), StorageError> {
        lifecycle::tx_commit_impl(self, snapshot)
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
mod tests;
