// `GraphStore` trait implementation for `FabricGraphStore`, decomposed into
// focused submodules.
//
// Rust requires trait impls to live in a single block, so the `impl GraphStore`
// for `FabricGraphStore` lives here. Every method is a thin delegator to a
// `*_impl` helper organized by topic:
//
// - helpers.rs:     shared inherent methods (id allocation, label refresh)
//                   and pure label-string builders
// - nodes.rs:       Cypher query builders for node operations
// - node_reads.rs:  node + adjacency read `*_impl` functions
// - node_writes.rs: node create / update / delete `*_impl` functions
// - edges.rs:       edge query builders + `*_impl` functions
// - admin.rs:       index and graph administration `*_impl` functions

pub(crate) mod admin;
pub(crate) mod edges;
pub(crate) mod helpers;
pub(crate) mod node_reads;
pub(crate) mod node_writes;
pub(crate) mod nodes;

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor, NodeData, NodeId,
    RangeQuery,
};
use cypher_core::types::Value;

use crate::FabricGraphStore;

impl GraphStore for FabricGraphStore {
    // ── Node reads ──────────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        node_reads::get_node_impl(self, graph, id)
    }

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        node_reads::get_nodes_batch_impl(self, graph, ids)
    }

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        node_reads::find_nodes_by_label_impl(self, graph, label)
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        node_reads::find_nodes_by_label_and_property_impl(self, graph, label, property, range)
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        node_reads::all_node_ids_impl(self, graph)
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        node_reads::all_label_names_impl(self, graph)
    }

    // ── Adjacency reads ─────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        node_reads::out_edges_impl(self, graph, node_id)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        node_reads::in_edges_impl(self, graph, node_id)
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        node_reads::out_edges_by_type_impl(self, graph, node_id, rel_types)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        node_reads::in_edges_by_type_impl(self, graph, node_id, rel_types)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        node_reads::out_edge_ids_impl(self, graph, node_id)
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        node_reads::in_edge_ids_impl(self, graph, node_id)
    }

    // ── Node writes ─────────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        node_writes::create_node_impl(self, graph, labels, properties)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        node_writes::set_node_properties_impl(self, graph, id, properties)
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        node_writes::replace_node_properties_impl(self, graph, id, properties)
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        node_writes::remove_node_property_impl(self, graph, id, key)
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        node_writes::add_node_labels_impl(self, graph, id, labels)
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        node_writes::remove_node_labels_impl(self, graph, id, labels)
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        node_writes::delete_node_impl(self, graph, id, detach)
    }

    // ── Edge operations (delegated to edges.rs) ─────────────────────────────

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        edges::get_edge_impl(self, graph, id)
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        edges::get_edges_batch_impl(self, graph, ids)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        edges::find_edges_by_type_impl(self, graph, rel_type)
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        edges::all_edge_ids_impl(self, graph)
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

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        edges::remove_edge_property_impl(self, graph, id, key)
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        edges::delete_edge_impl(self, graph, id)
    }

    // ── Index management (delegated to admin.rs) ────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        admin::list_indexes_impl(self, graph)
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        admin::create_index_impl(self, graph, label, property)
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        admin::drop_index_impl(self, graph, label, property)
    }

    // ── Graph administration (delegated to admin.rs) ────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        admin::create_graph_impl(self, name)
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        admin::delete_graph_impl(self, name)
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        admin::graph_exists_impl(self, name)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        admin::list_graphs_impl(self)
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        admin::graph_stats_impl(self, graph)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
