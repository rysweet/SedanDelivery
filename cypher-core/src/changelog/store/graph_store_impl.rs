//! [`GraphStore`] trait implementation for [`ChangeLogStore`].
//!
//! Read methods delegate straight to the inner store. Write methods delegate
//! to the inner store and, on success, push a [`ChangeEvent`] onto the
//! pending queue.

use indexmap::IndexMap;

use crate::error::StorageError;
use crate::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor, NodeData, NodeId,
    RangeQuery,
};
use crate::types::Value;

use super::super::event::ChangeEvent;
use super::super::props::props_to_json_map;
use super::ChangeLogStore;

impl<S: GraphStore + 'static> GraphStore for ChangeLogStore<S> {
    // ── Reads: straight delegation ────────────────────────────────────────────

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

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.inner.all_label_names(graph)
    }

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.inner.list_indexes(graph)
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

    // ── Writes: delegate then record event ───────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        let id = self
            .inner
            .create_node(graph, labels.clone(), properties.clone())?;
        self.push(ChangeEvent::CreateNode {
            graph: graph.to_string(),
            id: id.0,
            labels,
            properties: props_to_json_map(&properties),
        });
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
        self.push(ChangeEvent::CreateEdge {
            graph: graph.to_string(),
            id: id.0,
            src: src.0,
            dst: dst.0,
            rel_type,
            properties: props_to_json_map(&properties),
        });
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
        self.push(ChangeEvent::SetNodeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json_map(&properties),
        });
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
        self.push(ChangeEvent::ReplaceNodeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json_map(&properties),
        });
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
        self.push(ChangeEvent::SetEdgeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json_map(&properties),
        });
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
        self.push(ChangeEvent::ReplaceEdgeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json_map(&properties),
        });
        Ok(())
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.inner.remove_node_property(graph, id, key)?;
        self.push(ChangeEvent::RemoveNodeProperty {
            graph: graph.to_string(),
            id: id.0,
            key: key.to_string(),
        });
        Ok(())
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.inner.remove_edge_property(graph, id, key)?;
        self.push(ChangeEvent::RemoveEdgeProperty {
            graph: graph.to_string(),
            id: id.0,
            key: key.to_string(),
        });
        Ok(())
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.inner.add_node_labels(graph, id, labels)?;
        self.push(ChangeEvent::AddNodeLabels {
            graph: graph.to_string(),
            id: id.0,
            labels: labels.to_vec(),
        });
        Ok(())
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.inner.remove_node_labels(graph, id, labels)?;
        self.push(ChangeEvent::RemoveNodeLabels {
            graph: graph.to_string(),
            id: id.0,
            labels: labels.to_vec(),
        });
        Ok(())
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        self.inner.delete_node(graph, id, detach)?;
        self.push(ChangeEvent::DeleteNode {
            graph: graph.to_string(),
            id: id.0,
            detach,
        });
        Ok(())
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.inner.delete_edge(graph, id)?;
        self.push(ChangeEvent::DeleteEdge {
            graph: graph.to_string(),
            id: id.0,
        });
        Ok(())
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.inner.create_index(graph, label, property)?;
        self.push(ChangeEvent::CreateIndex {
            graph: graph.to_string(),
            label: label.to_string(),
            property: property.to_string(),
        });
        Ok(())
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.inner.drop_index(graph, label, property)?;
        self.push(ChangeEvent::DropIndex {
            graph: graph.to_string(),
            label: label.to_string(),
            property: property.to_string(),
        });
        Ok(())
    }

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        self.inner.create_graph(name)?;
        self.push(ChangeEvent::CreateGraph {
            name: name.to_string(),
        });
        Ok(())
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.inner.delete_graph(name)?;
        self.push(ChangeEvent::DeleteGraph {
            name: name.to_string(),
        });
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
