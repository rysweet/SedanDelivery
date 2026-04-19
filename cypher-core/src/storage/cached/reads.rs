//! Read-path inherent helpers for [`CachedGraphStore`].
//!
//! Each helper ensures the requested graph is loaded into the cache, then
//! delegates to the in-memory cache. The corresponding `GraphStore` trait
//! methods in [`super::store_impl`] are thin wrappers over these helpers.

use crate::error::StorageError;
use crate::storage::{
    AdjEntry, ConstraintDescriptor, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery,
};

use super::CachedGraphStore;

impl CachedGraphStore {
    pub(super) fn read_get_node(
        &self,
        graph: &str,
        id: NodeId,
    ) -> Result<Option<NodeData>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.get_node(graph, id)
    }

    pub(super) fn read_get_edge(
        &self,
        graph: &str,
        id: EdgeId,
    ) -> Result<Option<EdgeData>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.get_edge(graph, id)
    }

    pub(super) fn read_get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.get_nodes_batch(graph, ids)
    }

    pub(super) fn read_get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.get_edges_batch(graph, ids)
    }

    pub(super) fn read_find_nodes_by_label(
        &self,
        graph: &str,
        label: &str,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.find_nodes_by_label(graph, label)
    }

    pub(super) fn read_find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache
            .find_nodes_by_label_and_property(graph, label, property, range)
    }

    pub(super) fn read_find_edges_by_type(
        &self,
        graph: &str,
        rel_type: &str,
    ) -> Result<Vec<EdgeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.find_edges_by_type(graph, rel_type)
    }

    pub(super) fn read_out_edges(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.out_edges(graph, node_id)
    }

    pub(super) fn read_in_edges(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.in_edges(graph, node_id)
    }

    pub(super) fn read_out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.out_edges_by_type(graph, node_id, rel_types)
    }

    pub(super) fn read_in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.in_edges_by_type(graph, node_id, rel_types)
    }

    pub(super) fn read_out_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache
            .out_edges_by_type_batch(graph, node_ids, rel_types)
    }

    pub(super) fn read_in_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache
            .in_edges_by_type_batch(graph, node_ids, rel_types)
    }

    pub(super) fn read_out_edge_ids(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<EdgeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.out_edge_ids(graph, node_id)
    }

    pub(super) fn read_in_edge_ids(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<EdgeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.in_edge_ids(graph, node_id)
    }

    pub(super) fn read_all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.all_node_ids(graph)
    }

    pub(super) fn read_all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.all_edge_ids(graph)
    }

    pub(super) fn read_list_indexes(
        &self,
        graph: &str,
    ) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.list_indexes(graph)
    }

    pub(super) fn read_list_constraints(
        &self,
        graph: &str,
    ) -> Result<Vec<ConstraintDescriptor>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.list_constraints(graph)
    }

    pub(super) fn read_graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.graph_stats(graph)
    }

    pub(super) fn read_all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.all_label_names(graph)
    }
}
