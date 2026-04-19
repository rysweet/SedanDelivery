//! `GraphStore` trait implementation for [`CachedGraphStore`].
//!
//! Rust requires a single `impl GraphStore for CachedGraphStore` block, so
//! the trait surface lives here. Each method is a thin wrapper that delegates
//! to an inherent helper defined on a sibling module:
//!
//! - read-path helpers in [`super::reads`]
//! - write-path helpers in [`super::writes`]
//! - schema and graph-administration helpers in [`super::schema_admin`]

use std::any::Any;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::error::StorageError;
use crate::storage::{
    AdjEntry, ConstraintDescriptor, ConstraintType, EdgeData, EdgeId, GraphStats, GraphStore,
    IndexDescriptor, NodeData, NodeId, RangeQuery, ReadConsistency,
};
use crate::types::Value;

use super::CachedGraphStore;

impl GraphStore for CachedGraphStore {
    // ── Single-item reads ────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        self.read_get_node(graph, id)
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        self.read_get_edge(graph, id)
    }

    // ── Batch reads ──────────────────────────────────────────────────────────

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        self.read_get_nodes_batch(graph, ids)
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        self.read_get_edges_batch(graph, ids)
    }

    // ── Label/type index reads ───────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        self.read_find_nodes_by_label(graph, label)
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.read_find_nodes_by_label_and_property(graph, label, property, range)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.read_find_edges_by_type(graph, rel_type)
    }

    // ── Adjacency reads ──────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.read_out_edges(graph, node_id)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.read_in_edges(graph, node_id)
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.read_out_edges_by_type(graph, node_id, rel_types)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.read_in_edges_by_type(graph, node_id, rel_types)
    }

    fn out_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.read_out_edges_by_type_batch(graph, node_ids, rel_types)
    }

    fn in_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.read_in_edges_by_type_batch(graph, node_ids, rel_types)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.read_out_edge_ids(graph, node_id)
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.read_in_edge_ids(graph, node_id)
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        self.read_all_node_ids(graph)
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.read_all_edge_ids(graph)
    }

    // ── Write operations (write-through + invalidate) ────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        self.write_create_node(graph, labels, properties)
    }

    fn batch_create_nodes(
        &self,
        graph: &str,
        nodes: Vec<(Vec<String>, IndexMap<String, Value>)>,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.write_batch_create_nodes(graph, nodes)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.write_create_edge(graph, src, dst, rel_type, properties)
    }

    fn batch_create_edges(
        &self,
        graph: &str,
        edges: Vec<(NodeId, NodeId, String, IndexMap<String, Value>)>,
    ) -> Result<Vec<EdgeId>, StorageError> {
        self.write_batch_create_edges(graph, edges)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.write_set_node_properties(graph, id, properties)
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.write_replace_node_properties(graph, id, properties)
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.write_set_edge_properties(graph, id, properties)
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.write_replace_edge_properties(graph, id, properties)
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.write_remove_node_property(graph, id, key)
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.write_remove_edge_property(graph, id, key)
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.write_add_node_labels(graph, id, labels)
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.write_remove_node_labels(graph, id, labels)
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        self.write_delete_node(graph, id, detach)
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.write_delete_edge(graph, id)
    }

    // ── Index management ─────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.read_list_indexes(graph)
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.schema_create_index(graph, label, property)
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.schema_drop_index(graph, label, property)
    }

    // ── Constraint management ────────────────────────────────────────────────

    fn list_constraints(&self, graph: &str) -> Result<Vec<ConstraintDescriptor>, StorageError> {
        self.read_list_constraints(graph)
    }

    fn create_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        constraint_type: ConstraintType,
    ) -> Result<(), StorageError> {
        self.schema_create_constraint(graph, label, property, constraint_type)
    }

    fn drop_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.schema_drop_constraint(graph, label, property)
    }

    // ── Graph administration ─────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        self.admin_create_graph(name)
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.admin_delete_graph(name)
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        self.admin_graph_exists(name)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.admin_list_graphs()
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.read_graph_stats(graph)
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.read_all_label_names(graph)
    }

    fn with_read_consistency(&self, _consistency: ReadConsistency) -> Option<Arc<dyn GraphStore>> {
        // CachedGraphStore doesn't support topology-aware reads.
        None
    }

    // ── Transaction support ──────────────────────────────────────────────────

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn tx_snapshot(&self, _graphs: &[&str]) -> Result<Option<Box<dyn GraphStore>>, StorageError> {
        // Delegate to backend — cache is not transactional.
        Ok(None)
    }

    fn tx_commit(&self, _snapshot: &dyn GraphStore) -> Result<(), StorageError> {
        Ok(())
    }
}
