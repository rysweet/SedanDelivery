// graph_store — single `impl GraphStore for FanoutStore` block.
//
// Rust requires that there be exactly one trait impl per (Trait, Type) pair,
// so all `GraphStore` methods live in this file. Topics:
//   - Reads (lines beginning with `self.read(...)`) — primary-only.
//   - Mutations (`self.write_all(...)`) — fan out to every backend.
//   - `create_node` / `create_edge` — primary-assigned IDs propagated to
//     secondaries via reserved `_sd2_id` / `_sd2_eid` properties, with
//     best-effort rollback on partial failure.

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor, NodeData, NodeId,
    RangeQuery,
};
use cypher_core::types::Value;

use super::FanoutStore;

// Note: batch_create_nodes and batch_create_edges use the default trait
// implementation (single-item delegation) because FanoutStore's cross-backend
// rollback logic operates at the individual create level.
// Note: batch_create_nodes and batch_create_edges use the default trait
// implementation (single-item delegation) because FanoutStore's cross-backend
// rollback logic operates at the individual create level.
impl GraphStore for FanoutStore {
    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        self.read(|s| s.get_node(graph, id))
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        self.read(|s| s.get_edge(graph, id))
    }

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        self.read(|s| s.get_nodes_batch(graph, ids))
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        self.read(|s| s.get_edges_batch(graph, ids))
    }

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        self.read(|s| s.find_nodes_by_label(graph, label))
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        // Read from primary only (RangeQuery holds references, can't fanout writes).
        self.stores[0].find_nodes_by_label_and_property(graph, label, property, range)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.read(|s| s.find_edges_by_type(graph, rel_type))
    }

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.read(|s| s.out_edges(graph, node_id))
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.read(|s| s.in_edges(graph, node_id))
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.read(|s| s.out_edges_by_type(graph, node_id, rel_types))
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.read(|s| s.in_edges_by_type(graph, node_id, rel_types))
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.read(|s| s.out_edge_ids(graph, node_id))
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.read(|s| s.in_edge_ids(graph, node_id))
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        self.read(|s| s.all_node_ids(graph))
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.read(|s| s.all_edge_ids(graph))
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.read(|s| s.all_label_names(graph))
    }

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        // Assign ID from the primary store, then tell secondaries exactly which
        // ID to use by injecting `_sd2_id` into the property map.  This keeps
        // IDs consistent across all backends (e.g. Redis primary + Fabric secondary).
        let id = self.stores[0].create_node(graph, labels.clone(), properties.clone())?;
        let mut props_with_id = properties.clone();
        let id_i64 = i64::try_from(id.0).map_err(|_| {
            crate::record_backend_error();
            StorageError::Backend {
                msg: format!("node ID {} exceeds i64::MAX", id.0),
            }
        })?;
        props_with_id.insert("_sd2_id".to_string(), Value::Int(id_i64));
        let mut successful_secondaries: Vec<usize> = Vec::new();
        for (offset, store) in self.stores[1..].iter().enumerate() {
            if let Err(e) = store.create_node(graph, labels.clone(), props_with_id.clone()) {
                let failed_store_index = offset + 1;
                tracing::error!(
                    node_id = %id.0,
                    failed_store_index,
                    error = %e,
                    "secondary store failed to create node, attempting rollback on successful stores"
                );
                for secondary_index in successful_secondaries.iter().copied().rev() {
                    if let Err(rollback_err) =
                        self.stores[secondary_index].delete_node(graph, id, true)
                    {
                        tracing::error!(
                            node_id = %id.0,
                            secondary_index,
                            error = %rollback_err,
                            "rollback failed: could not delete node from a previously successful secondary"
                        );
                    }
                }
                // Attempt to delete the node from the primary to maintain consistency.
                if let Err(rollback_err) = self.stores[0].delete_node(graph, id, true) {
                    tracing::error!(
                        node_id = %id.0,
                        error = %rollback_err,
                        "rollback failed: could not delete node from primary after secondary write failure"
                    );
                }
                return Err(e);
            }
            successful_secondaries.push(offset + 1);
        }
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
        let id =
            self.stores[0].create_edge(graph, src, dst, rel_type.clone(), properties.clone())?;
        let mut props_with_id = properties.clone();
        let id_i64 = i64::try_from(id.0).map_err(|_| {
            crate::record_backend_error();
            StorageError::Backend {
                msg: format!("edge ID {} exceeds i64::MAX", id.0),
            }
        })?;
        props_with_id.insert("_sd2_eid".to_string(), Value::Int(id_i64));
        let mut successful_secondaries: Vec<usize> = Vec::new();
        for (offset, store) in self.stores[1..].iter().enumerate() {
            if let Err(e) =
                store.create_edge(graph, src, dst, rel_type.clone(), props_with_id.clone())
            {
                let failed_store_index = offset + 1;
                tracing::error!(
                    edge_id = %id.0,
                    failed_store_index,
                    error = %e,
                    "secondary store failed to create edge, attempting rollback on successful stores"
                );
                for secondary_index in successful_secondaries.iter().copied().rev() {
                    if let Err(rollback_err) = self.stores[secondary_index].delete_edge(graph, id) {
                        tracing::error!(
                            edge_id = %id.0,
                            secondary_index,
                            error = %rollback_err,
                            "rollback failed: could not delete edge from a previously successful secondary"
                        );
                    }
                }
                // Attempt to delete the edge from the primary to maintain consistency.
                if let Err(rollback_err) = self.stores[0].delete_edge(graph, id) {
                    tracing::error!(
                        edge_id = %id.0,
                        error = %rollback_err,
                        "rollback failed: could not delete edge from primary after secondary write failure"
                    );
                }
                return Err(e);
            }
            successful_secondaries.push(offset + 1);
        }
        Ok(id)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.write_all(|s| s.set_node_properties(graph, id, properties.clone()))
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.write_all(|s| s.replace_node_properties(graph, id, properties.clone()))
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.write_all(|s| s.set_edge_properties(graph, id, properties.clone()))
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.write_all(|s| s.replace_edge_properties(graph, id, properties.clone()))
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.write_all(|s| s.remove_node_property(graph, id, key))
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.write_all(|s| s.remove_edge_property(graph, id, key))
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.write_all(|s| s.add_node_labels(graph, id, labels))
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.write_all(|s| s.remove_node_labels(graph, id, labels))
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        self.write_all(|s| s.delete_node(graph, id, detach))
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.write_all(|s| s.delete_edge(graph, id))
    }

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.read(|s| s.list_indexes(graph))
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.write_all(|s| s.create_index(graph, label, property))
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.write_all(|s| s.drop_index(graph, label, property))
    }

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        self.write_all(|s| s.create_graph(name))
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.write_all(|s| s.delete_graph(name))
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        self.read(|s| s.graph_exists(name))
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.read(|s| s.list_graphs())
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.read(|s| s.graph_stats(graph))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
