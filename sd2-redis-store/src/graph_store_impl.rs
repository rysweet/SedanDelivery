// `impl GraphStore for RedisGraphStore` — the trait wiring.
//
// All methods delegate to per-concern `_impl` helpers defined in `reads.rs`,
// `writes/`, `indexing.rs`, `range.rs`, and `audit.rs`. Keeping the trait
// surface here makes the inventory of supported operations easy to read at
// a glance and isolates trait-shape churn from the per-method business logic.

use std::sync::Arc;

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor, NodeData, NodeId,
    RangeQuery, ReadConsistency,
};
use cypher_core::types::Value;

use crate::RedisGraphStore;

impl GraphStore for RedisGraphStore {
    // ── Single-item reads ────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        self.get_node_impl(graph, id)
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        self.get_edge_impl(graph, id)
    }

    // ── Batch reads ──────────────────────────────────────────────────────────

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        self.get_nodes_batch_impl(graph, ids)
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        self.get_edges_batch_impl(graph, ids)
    }

    // ── Label/type index reads ───────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        self.find_nodes_by_label_impl(graph, label)
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.find_nodes_by_label_and_property_impl(graph, label, property, range)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.find_edges_by_type_impl(graph, rel_type)
    }

    // ── Adjacency reads ─────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.out_edges_impl(graph, node_id)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.in_edges_impl(graph, node_id)
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.out_edges_by_type_impl(graph, node_id, rel_types)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.in_edges_by_type_impl(graph, node_id, rel_types)
    }

    fn out_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.out_edges_by_type_batch_impl(graph, node_ids, rel_types)
    }

    fn in_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.in_edges_by_type_batch_impl(graph, node_ids, rel_types)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.out_edge_ids_impl(graph, node_id)
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.in_edge_ids_impl(graph, node_id)
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        self.all_node_ids_impl(graph)
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.all_edge_ids_impl(graph)
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.all_label_names_impl(graph)
    }

    // ── Write operations ────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        self.create_node_impl(graph, labels, properties)
    }

    fn batch_create_nodes(
        &self,
        graph: &str,
        nodes: Vec<(Vec<String>, IndexMap<String, Value>)>,
    ) -> Result<Vec<NodeId>, StorageError> {
        RedisGraphStore::batch_create_nodes(self, graph, nodes)
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

    fn batch_create_edges(
        &self,
        graph: &str,
        edges: Vec<(NodeId, NodeId, String, IndexMap<String, Value>)>,
    ) -> Result<Vec<EdgeId>, StorageError> {
        RedisGraphStore::batch_create_edges(self, graph, edges)
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

    // ── Index management ────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.list_indexes_impl(graph)
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.create_index_impl(graph, label, property)
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.drop_index_impl(graph, label, property)
    }

    // ── Graph administration ────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        self.create_graph_impl(name)
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.delete_graph_impl(name)
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        self.graph_exists_impl(name)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.list_graphs_impl()
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.graph_stats_impl(graph)
    }

    fn with_read_consistency(&self, consistency: ReadConsistency) -> Option<Arc<dyn GraphStore>> {
        Some(Arc::new(
            self.clone().with_read_consistency_mode(consistency),
        ))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    // tx_snapshot and tx_commit use the default no-op implementations.
    // RedisGraphStore writes are immediately durable — no client-side
    // snapshot is needed. Redis Cluster mode can add WATCH/MULTI/EXEC
    // for per-slot transaction isolation in a future iteration.
}
