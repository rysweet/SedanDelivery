// fanout.rs — FanoutStore: write to all backends, read from the first.
//
// Typical use: FanoutStore { stores: [RedisGraphStore, InMemoryGraphStore] }
// ensures all writes reach both backends while reads always come from Redis.

use std::collections::HashMap;

use gql_core::error::StorageError;
use gql_core::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor, NodeData, NodeId,
    RangeQuery,
};
use gql_core::types::Value;

/// Writes to ALL backing stores simultaneously; reads from the first one.
///
/// If any write fails, the error from that store is returned immediately.
/// The caller must choose a suitable retry/rollback strategy.
pub struct FanoutStore {
    stores: Vec<Box<dyn GraphStore>>,
}

impl FanoutStore {
    /// Create a fanout store over `stores`.
    ///
    /// # Panics
    /// Panics if `stores` is empty — a fanout over zero backends makes no sense.
    pub fn new(stores: Vec<Box<dyn GraphStore>>) -> Self {
        assert!(
            !stores.is_empty(),
            "FanoutStore requires at least one backend"
        );
        Self { stores }
    }

    /// Apply a read operation against the primary (first) store.
    fn read<T, F>(&self, f: F) -> Result<T, StorageError>
    where
        F: Fn(&dyn GraphStore) -> Result<T, StorageError>,
    {
        f(self.stores[0].as_ref())
    }

    /// Apply a write operation against all stores. Returns on the first error.
    fn write_all<F>(&self, f: F) -> Result<(), StorageError>
    where
        F: Fn(&dyn GraphStore) -> Result<(), StorageError>,
    {
        for store in &self.stores {
            f(store.as_ref())?;
        }
        Ok(())
    }

    /// Apply a write that returns a value from each store; returns the primary's result.
    #[allow(dead_code)] // Available for future use by new GraphStore methods returning values
    fn write_all_primary<T: Clone, F>(&self, f: F) -> Result<T, StorageError>
    where
        F: Fn(&dyn GraphStore) -> Result<T, StorageError>,
    {
        let mut primary_result: Option<T> = None;
        for (i, store) in self.stores.iter().enumerate() {
            let result = f(store.as_ref())?;
            if i == 0 {
                primary_result = Some(result);
            }
        }
        Ok(primary_result.expect("stores is non-empty"))
    }
}

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
        properties: HashMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        // Assign ID from the primary store, then tell secondaries exactly which
        // ID to use by injecting `_sd2_id` into the property map.  This keeps
        // IDs consistent across all backends (e.g. Redis primary + Fabric secondary).
        let id = self.stores[0].create_node(graph, labels.clone(), properties.clone())?;
        let mut props_with_id = properties.clone();
        props_with_id.insert("_sd2_id".to_string(), Value::Int(id.0 as i64));
        for store in &self.stores[1..] {
            store.create_node(graph, labels.clone(), props_with_id.clone())?;
        }
        Ok(id)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: HashMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        let id =
            self.stores[0].create_edge(graph, src, dst, rel_type.clone(), properties.clone())?;
        let mut props_with_id = properties.clone();
        props_with_id.insert("_sd2_eid".to_string(), Value::Int(id.0 as i64));
        for store in &self.stores[1..] {
            store.create_edge(graph, src, dst, rel_type.clone(), props_with_id.clone())?;
        }
        Ok(id)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.write_all(|s| s.set_node_properties(graph, id, properties.clone()))
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.write_all(|s| s.replace_node_properties(graph, id, properties.clone()))
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.write_all(|s| s.set_edge_properties(graph, id, properties.clone()))
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
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
}
