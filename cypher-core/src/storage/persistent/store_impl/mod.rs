//! `GraphStore` trait implementation for [`PersistentGraphStore`].
//!
//! Reads delegate directly to the in-memory store. Writes apply to the
//! in-memory store first, then append a WAL entry. Write logic lives in the
//! sibling modules ([`nodes`], [`edges`], [`indexes`]) as inherent helpers
//! that this trait impl forwards to.

mod edges;
mod indexes;
mod nodes;

use std::any::Any;
use std::sync::Arc;

use indexmap::IndexMap;

use super::PersistentGraphStore;
use crate::error::StorageError;
use crate::storage::{
    AdjEntry, ConstraintDescriptor, ConstraintType, EdgeData, EdgeId, GraphStats, GraphStore,
    IndexDescriptor, NodeData, NodeId, RangeQuery, ReadConsistency,
};
use crate::types::Value;

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

    // ── Writes (delegate to inherent helpers in sibling modules) ─────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        self.create_graph_impl(name)
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.delete_graph_impl(name)
    }

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        self.create_node_impl(graph, labels, properties)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.create_edge_impl(graph, src, dst, rel_type, properties)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.set_node_properties_impl(graph, id, properties)
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.replace_node_properties_impl(graph, id, properties)
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.set_edge_properties_impl(graph, id, properties)
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.replace_edge_properties_impl(graph, id, properties)
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.remove_node_property_impl(graph, id, key)
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.remove_edge_property_impl(graph, id, key)
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.add_node_labels_impl(graph, id, labels)
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.remove_node_labels_impl(graph, id, labels)
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        self.delete_node_impl(graph, id, detach)
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.delete_edge_impl(graph, id)
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.create_index_impl(graph, label, property)
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.drop_index_impl(graph, label, property)
    }

    fn create_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        constraint_type: ConstraintType,
    ) -> Result<(), StorageError> {
        self.create_constraint_impl(graph, label, property, constraint_type)
    }

    fn drop_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.drop_constraint_impl(graph, label, property)
    }
}
