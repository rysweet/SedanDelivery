//! Write-path inherent helpers for [`CachedGraphStore`].
//!
//! Each helper applies the mutation to the backend first, then mirrors the
//! change into the cache via the helpers in [`super::invalidation`]
//! (write-through). The corresponding `GraphStore` trait methods in
//! [`super::store_impl`] are thin wrappers over these helpers.

use indexmap::IndexMap;

use crate::error::StorageError;
use crate::storage::{EdgeData, EdgeId, NodeData, NodeId};
use crate::types::Value;

use super::CachedGraphStore;

impl CachedGraphStore {
    pub(super) fn write_create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        let id = self
            .backend
            .create_node(graph, labels.clone(), properties.clone())?;
        if self.is_cached(graph) {
            let node = NodeData {
                id,
                labels,
                properties,
            };
            self.inject_node_into_cache(graph, node)?;
        }
        Ok(id)
    }

    pub(super) fn write_batch_create_nodes(
        &self,
        graph: &str,
        nodes: Vec<(Vec<String>, IndexMap<String, Value>)>,
    ) -> Result<Vec<NodeId>, StorageError> {
        let ids = self.backend.batch_create_nodes(graph, nodes.clone())?;
        if self.is_cached(graph) {
            for (id, (labels, properties)) in ids.iter().zip(nodes) {
                let node = NodeData {
                    id: *id,
                    labels,
                    properties,
                };
                self.inject_node_into_cache(graph, node)?;
            }
        }
        Ok(ids)
    }

    pub(super) fn write_create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        let id = self
            .backend
            .create_edge(graph, src, dst, rel_type.clone(), properties.clone())?;
        if self.is_cached(graph) {
            let edge = EdgeData {
                id,
                src,
                dst,
                rel_type,
                properties,
            };
            self.inject_edge_into_cache(graph, edge)?;
        }
        Ok(id)
    }

    pub(super) fn write_batch_create_edges(
        &self,
        graph: &str,
        edges: Vec<(NodeId, NodeId, String, IndexMap<String, Value>)>,
    ) -> Result<Vec<EdgeId>, StorageError> {
        let ids = self.backend.batch_create_edges(graph, edges.clone())?;
        if self.is_cached(graph) {
            for (id, (src, dst, rel_type, properties)) in ids.iter().zip(edges) {
                let edge = EdgeData {
                    id: *id,
                    src,
                    dst,
                    rel_type,
                    properties,
                };
                self.inject_edge_into_cache(graph, edge)?;
            }
        }
        Ok(ids)
    }

    pub(super) fn write_set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.backend
            .set_node_properties(graph, id, properties.clone())?;
        if self.is_cached(graph) {
            self.update_node_props_in_cache(graph, id, &properties)?;
        }
        Ok(())
    }

    pub(super) fn write_replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.backend
            .replace_node_properties(graph, id, properties.clone())?;
        if self.is_cached(graph) {
            self.replace_node_props_in_cache(graph, id, &properties)?;
        }
        Ok(())
    }

    pub(super) fn write_set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.backend
            .set_edge_properties(graph, id, properties.clone())?;
        if self.is_cached(graph) {
            self.update_edge_props_in_cache(graph, id, &properties)?;
        }
        Ok(())
    }

    pub(super) fn write_replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.backend
            .replace_edge_properties(graph, id, properties.clone())?;
        if self.is_cached(graph) {
            self.replace_edge_props_in_cache(graph, id, &properties)?;
        }
        Ok(())
    }

    pub(super) fn write_remove_node_property(
        &self,
        graph: &str,
        id: NodeId,
        key: &str,
    ) -> Result<(), StorageError> {
        self.backend.remove_node_property(graph, id, key)?;
        if self.is_cached(graph) {
            self.remove_node_prop_in_cache(graph, id, key)?;
        }
        Ok(())
    }

    pub(super) fn write_remove_edge_property(
        &self,
        graph: &str,
        id: EdgeId,
        key: &str,
    ) -> Result<(), StorageError> {
        self.backend.remove_edge_property(graph, id, key)?;
        if self.is_cached(graph) {
            self.remove_edge_prop_in_cache(graph, id, key)?;
        }
        Ok(())
    }

    pub(super) fn write_add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.backend.add_node_labels(graph, id, labels)?;
        if self.is_cached(graph) {
            self.add_labels_in_cache(graph, id, labels)?;
        }
        Ok(())
    }

    pub(super) fn write_remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.backend.remove_node_labels(graph, id, labels)?;
        if self.is_cached(graph) {
            self.remove_labels_in_cache(graph, id, labels)?;
        }
        Ok(())
    }

    pub(super) fn write_delete_node(
        &self,
        graph: &str,
        id: NodeId,
        detach: bool,
    ) -> Result<(), StorageError> {
        self.backend.delete_node(graph, id, detach)?;
        if self.is_cached(graph) {
            self.remove_node_from_cache(graph, id, detach)?;
        }
        Ok(())
    }

    pub(super) fn write_delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.backend.delete_edge(graph, id)?;
        if self.is_cached(graph) {
            self.remove_edge_from_cache(graph, id)?;
        }
        Ok(())
    }
}
