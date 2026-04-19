//! `GraphStore` trait implementation for `CachedGraphStore`.
//!
//! Reads call `ensure_loaded` then delegate to the in-memory cache.
//! Writes apply to the backend first, then mirror the change into the
//! cache via the helpers in [`super::invalidation`] (write-through).
//!
//! Rust requires a single `impl GraphStore for CachedGraphStore` block,
//! so reads and writes share this file.

use std::any::Any;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::error::StorageError;
use crate::storage::{
    validate_graph_name, AdjEntry, ConstraintDescriptor, ConstraintType, EdgeData, EdgeId,
    GraphStats, GraphStore, IndexDescriptor, NodeData, NodeId, RangeQuery, ReadConsistency,
};
use crate::types::Value;

use super::CachedGraphStore;

impl GraphStore for CachedGraphStore {
    // ── Single-item reads ────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.get_node(graph, id)
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.get_edge(graph, id)
    }

    // ── Batch reads ──────────────────────────────────────────────────────────

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.get_nodes_batch(graph, ids)
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.get_edges_batch(graph, ids)
    }

    // ── Label/type index reads ───────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.find_nodes_by_label(graph, label)
    }

    fn find_nodes_by_label_and_property(
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

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.find_edges_by_type(graph, rel_type)
    }

    // ── Adjacency reads ──────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.out_edges(graph, node_id)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.in_edges(graph, node_id)
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.out_edges_by_type(graph, node_id, rel_types)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.in_edges_by_type(graph, node_id, rel_types)
    }

    fn out_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache
            .out_edges_by_type_batch(graph, node_ids, rel_types)
    }

    fn in_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache
            .in_edges_by_type_batch(graph, node_ids, rel_types)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.out_edge_ids(graph, node_id)
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.in_edge_ids(graph, node_id)
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.all_node_ids(graph)
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.all_edge_ids(graph)
    }

    // ── Write operations (write-through + invalidate) ────────────────────────

    fn create_node(
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

    fn batch_create_nodes(
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

    fn create_edge(
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

    fn batch_create_edges(
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

    fn set_node_properties(
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

    fn replace_node_properties(
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

    fn set_edge_properties(
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

    fn replace_edge_properties(
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

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.backend.remove_node_property(graph, id, key)?;
        if self.is_cached(graph) {
            self.remove_node_prop_in_cache(graph, id, key)?;
        }
        Ok(())
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.backend.remove_edge_property(graph, id, key)?;
        if self.is_cached(graph) {
            self.remove_edge_prop_in_cache(graph, id, key)?;
        }
        Ok(())
    }

    fn add_node_labels(
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

    fn remove_node_labels(
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

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        self.backend.delete_node(graph, id, detach)?;
        if self.is_cached(graph) {
            self.remove_node_from_cache(graph, id, detach)?;
        }
        Ok(())
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.backend.delete_edge(graph, id)?;
        if self.is_cached(graph) {
            self.remove_edge_from_cache(graph, id)?;
        }
        Ok(())
    }

    // ── Index management ─────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.list_indexes(graph)
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.backend.create_index(graph, label, property)?;
        if self.is_cached(graph) {
            self.cache.create_index(graph, label, property)?;
        }
        Ok(())
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.backend.drop_index(graph, label, property)?;
        if self.is_cached(graph) {
            self.cache.drop_index(graph, label, property)?;
        }
        Ok(())
    }

    // ── Constraint management ────────────────────────────────────────────────

    fn list_constraints(&self, graph: &str) -> Result<Vec<ConstraintDescriptor>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.list_constraints(graph)
    }

    fn create_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        constraint_type: ConstraintType,
    ) -> Result<(), StorageError> {
        self.backend
            .create_constraint(graph, label, property, constraint_type.clone())?;
        if self.is_cached(graph) {
            self.cache
                .create_constraint(graph, label, property, constraint_type)?;
        }
        Ok(())
    }

    fn drop_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.backend.drop_constraint(graph, label, property)?;
        if self.is_cached(graph) {
            self.cache.drop_constraint(graph, label, property)?;
        }
        Ok(())
    }

    // ── Graph administration ─────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        self.backend.create_graph(name)?;
        // Don't auto-cache empty graphs; they'll be loaded on first access.
        Ok(())
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.backend.delete_graph(name)?;
        self.invalidate(name);
        Ok(())
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        // Check backend directly — don't require full load.
        self.backend.graph_exists(name)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.backend.list_graphs()
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.graph_stats(graph)
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.all_label_names(graph)
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
