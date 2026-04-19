//! `GraphStore` trait implementation for [`PersistentGraphStore`].
//!
//! Reads delegate directly to the in-memory store. Writes apply to the
//! in-memory store first, then append a WAL entry.

use std::any::Any;
use std::sync::Arc;

use indexmap::IndexMap;

use super::wal::{props_to_json, WalEntry};
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

    // ── Writes (delegate + WAL) ──────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        self.inner.create_graph(name)?;
        self.append_wal(&WalEntry::CreateGraph {
            name: name.to_string(),
        })?;
        Ok(())
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.inner.delete_graph(name)?;
        self.append_wal(&WalEntry::DeleteGraph {
            name: name.to_string(),
        })?;
        Ok(())
    }

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        let id = self
            .inner
            .create_node(graph, labels.clone(), properties.clone())?;
        self.append_wal(&WalEntry::CreateNode {
            graph: graph.to_string(),
            labels,
            properties: props_to_json(&properties),
            id: id.0,
        })?;
        Ok(id)
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
            .inner
            .create_edge(graph, src, dst, rel_type.clone(), properties.clone())?;
        self.append_wal(&WalEntry::CreateEdge {
            graph: graph.to_string(),
            src: src.0,
            dst: dst.0,
            rel_type,
            properties: props_to_json(&properties),
            id: id.0,
        })?;
        Ok(id)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .set_node_properties(graph, id, properties.clone())?;
        self.append_wal(&WalEntry::SetNodeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        Ok(())
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .replace_node_properties(graph, id, properties.clone())?;
        self.append_wal(&WalEntry::ReplaceNodeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        Ok(())
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .set_edge_properties(graph, id, properties.clone())?;
        self.append_wal(&WalEntry::SetEdgeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        Ok(())
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .replace_edge_properties(graph, id, properties.clone())?;
        self.append_wal(&WalEntry::ReplaceEdgeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        Ok(())
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.inner.remove_node_property(graph, id, key)?;
        self.append_wal(&WalEntry::RemoveNodeProperty {
            graph: graph.to_string(),
            id: id.0,
            key: key.to_string(),
        })?;
        Ok(())
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.inner.remove_edge_property(graph, id, key)?;
        self.append_wal(&WalEntry::RemoveEdgeProperty {
            graph: graph.to_string(),
            id: id.0,
            key: key.to_string(),
        })?;
        Ok(())
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.inner.add_node_labels(graph, id, labels)?;
        self.append_wal(&WalEntry::AddNodeLabels {
            graph: graph.to_string(),
            id: id.0,
            labels: labels.to_vec(),
        })?;
        Ok(())
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.inner.remove_node_labels(graph, id, labels)?;
        self.append_wal(&WalEntry::RemoveNodeLabels {
            graph: graph.to_string(),
            id: id.0,
            labels: labels.to_vec(),
        })?;
        Ok(())
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        // If detaching, we need to log edge deletions first so the WAL can replay correctly.
        if detach {
            let out_ids = self.inner.out_edge_ids(graph, id)?;
            let in_ids = self.inner.in_edge_ids(graph, id)?;
            self.inner.delete_node(graph, id, true)?;
            // Log each edge deletion, then the node deletion.
            for eid in out_ids {
                self.append_wal(&WalEntry::DeleteEdge {
                    graph: graph.to_string(),
                    id: eid.0,
                })?;
            }
            for eid in in_ids {
                self.append_wal(&WalEntry::DeleteEdge {
                    graph: graph.to_string(),
                    id: eid.0,
                })?;
            }
            self.append_wal(&WalEntry::DeleteNode {
                graph: graph.to_string(),
                id: id.0,
            })?;
        } else {
            self.inner.delete_node(graph, id, false)?;
            self.append_wal(&WalEntry::DeleteNode {
                graph: graph.to_string(),
                id: id.0,
            })?;
        }
        Ok(())
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.inner.delete_edge(graph, id)?;
        self.append_wal(&WalEntry::DeleteEdge {
            graph: graph.to_string(),
            id: id.0,
        })?;
        Ok(())
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.inner.create_index(graph, label, property)?;
        self.append_wal(&WalEntry::CreateIndex {
            graph: graph.to_string(),
            label: label.to_string(),
            property: property.to_string(),
        })?;
        Ok(())
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.inner.drop_index(graph, label, property)?;
        self.append_wal(&WalEntry::DropIndex {
            graph: graph.to_string(),
            label: label.to_string(),
            property: property.to_string(),
        })?;
        Ok(())
    }

    fn create_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        constraint_type: ConstraintType,
    ) -> Result<(), StorageError> {
        self.inner
            .create_constraint(graph, label, property, constraint_type)
    }

    fn drop_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.inner.drop_constraint(graph, label, property)
    }
}
