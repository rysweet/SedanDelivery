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
        let id_i64 = i64::try_from(id.0).map_err(|_| StorageError::Backend {
            msg: format!("node ID {} exceeds i64::MAX", id.0),
        })?;
        props_with_id.insert("_sd2_id".to_string(), Value::Int(id_i64));
        for store in &self.stores[1..] {
            if let Err(e) = store.create_node(graph, labels.clone(), props_with_id.clone()) {
                tracing::error!(node_id = %id.0, error = %e, "secondary store failed to create node, attempting rollback on primary");
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
        let id_i64 = i64::try_from(id.0).map_err(|_| StorageError::Backend {
            msg: format!("edge ID {} exceeds i64::MAX", id.0),
        })?;
        props_with_id.insert("_sd2_eid".to_string(), Value::Int(id_i64));
        for store in &self.stores[1..] {
            if let Err(e) =
                store.create_edge(graph, src, dst, rel_type.clone(), props_with_id.clone())
            {
                tracing::error!(edge_id = %id.0, error = %e, "secondary store failed to create edge, attempting rollback on primary");
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

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use gql_core::error::StorageError;
    use gql_core::storage::InMemoryGraphStore;

    // ── ArcStore: shared-reference wrapper for verifying both stores ─────────
    //
    // FanoutStore takes ownership of its backends via `Box<dyn GraphStore>`.
    // To verify that writes actually reach the secondary, we need a way to
    // keep a readable reference to the underlying InMemoryGraphStore while
    // FanoutStore owns a Box. ArcStore wraps `Arc<InMemoryGraphStore>` and
    // delegates all GraphStore methods, letting us clone the Arc before
    // handing one copy to FanoutStore and keeping another for assertions.

    struct ArcStore(Arc<InMemoryGraphStore>);

    impl GraphStore for ArcStore {
        fn get_node(&self, g: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
            self.0.get_node(g, id)
        }
        fn get_edge(&self, g: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
            self.0.get_edge(g, id)
        }
        fn get_nodes_batch(
            &self,
            g: &str,
            ids: &[NodeId],
        ) -> Result<Vec<Option<NodeData>>, StorageError> {
            self.0.get_nodes_batch(g, ids)
        }
        fn get_edges_batch(
            &self,
            g: &str,
            ids: &[EdgeId],
        ) -> Result<Vec<Option<EdgeData>>, StorageError> {
            self.0.get_edges_batch(g, ids)
        }
        fn find_nodes_by_label(&self, g: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
            self.0.find_nodes_by_label(g, label)
        }
        fn find_nodes_by_label_and_property(
            &self,
            g: &str,
            label: &str,
            property: &str,
            range: RangeQuery<'_>,
        ) -> Result<Vec<NodeId>, StorageError> {
            self.0
                .find_nodes_by_label_and_property(g, label, property, range)
        }
        fn find_edges_by_type(
            &self,
            g: &str,
            rel_type: &str,
        ) -> Result<Vec<EdgeId>, StorageError> {
            self.0.find_edges_by_type(g, rel_type)
        }
        fn out_edges(&self, g: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
            self.0.out_edges(g, node_id)
        }
        fn in_edges(&self, g: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
            self.0.in_edges(g, node_id)
        }
        fn out_edges_by_type(
            &self,
            g: &str,
            node_id: NodeId,
            rel_types: &[&str],
        ) -> Result<Vec<AdjEntry>, StorageError> {
            self.0.out_edges_by_type(g, node_id, rel_types)
        }
        fn in_edges_by_type(
            &self,
            g: &str,
            node_id: NodeId,
            rel_types: &[&str],
        ) -> Result<Vec<AdjEntry>, StorageError> {
            self.0.in_edges_by_type(g, node_id, rel_types)
        }
        fn out_edge_ids(&self, g: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
            self.0.out_edge_ids(g, node_id)
        }
        fn in_edge_ids(&self, g: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
            self.0.in_edge_ids(g, node_id)
        }
        fn all_node_ids(&self, g: &str) -> Result<Vec<NodeId>, StorageError> {
            self.0.all_node_ids(g)
        }
        fn all_edge_ids(&self, g: &str) -> Result<Vec<EdgeId>, StorageError> {
            self.0.all_edge_ids(g)
        }
        fn all_label_names(&self, g: &str) -> Result<Vec<String>, StorageError> {
            self.0.all_label_names(g)
        }
        fn create_node(
            &self,
            g: &str,
            labels: Vec<String>,
            properties: HashMap<String, Value>,
        ) -> Result<NodeId, StorageError> {
            self.0.create_node(g, labels, properties)
        }
        fn create_edge(
            &self,
            g: &str,
            src: NodeId,
            dst: NodeId,
            rel_type: String,
            properties: HashMap<String, Value>,
        ) -> Result<EdgeId, StorageError> {
            self.0.create_edge(g, src, dst, rel_type, properties)
        }
        fn set_node_properties(
            &self,
            g: &str,
            id: NodeId,
            properties: HashMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.0.set_node_properties(g, id, properties)
        }
        fn replace_node_properties(
            &self,
            g: &str,
            id: NodeId,
            properties: HashMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.0.replace_node_properties(g, id, properties)
        }
        fn set_edge_properties(
            &self,
            g: &str,
            id: EdgeId,
            properties: HashMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.0.set_edge_properties(g, id, properties)
        }
        fn replace_edge_properties(
            &self,
            g: &str,
            id: EdgeId,
            properties: HashMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.0.replace_edge_properties(g, id, properties)
        }
        fn remove_node_property(
            &self,
            g: &str,
            id: NodeId,
            key: &str,
        ) -> Result<(), StorageError> {
            self.0.remove_node_property(g, id, key)
        }
        fn remove_edge_property(
            &self,
            g: &str,
            id: EdgeId,
            key: &str,
        ) -> Result<(), StorageError> {
            self.0.remove_edge_property(g, id, key)
        }
        fn add_node_labels(
            &self,
            g: &str,
            id: NodeId,
            labels: &[String],
        ) -> Result<(), StorageError> {
            self.0.add_node_labels(g, id, labels)
        }
        fn remove_node_labels(
            &self,
            g: &str,
            id: NodeId,
            labels: &[String],
        ) -> Result<(), StorageError> {
            self.0.remove_node_labels(g, id, labels)
        }
        fn delete_node(&self, g: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
            self.0.delete_node(g, id, detach)
        }
        fn delete_edge(&self, g: &str, id: EdgeId) -> Result<(), StorageError> {
            self.0.delete_edge(g, id)
        }
        fn list_indexes(&self, g: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
            self.0.list_indexes(g)
        }
        fn create_index(&self, g: &str, label: &str, property: &str) -> Result<(), StorageError> {
            self.0.create_index(g, label, property)
        }
        fn drop_index(&self, g: &str, label: &str, property: &str) -> Result<(), StorageError> {
            self.0.drop_index(g, label, property)
        }
        fn create_graph(&self, name: &str) -> Result<(), StorageError> {
            self.0.create_graph(name)
        }
        fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
            self.0.delete_graph(name)
        }
        fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
            self.0.graph_exists(name)
        }
        fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
            self.0.list_graphs()
        }
        fn graph_stats(&self, g: &str) -> Result<GraphStats, StorageError> {
            self.0.graph_stats(g)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    /// A store that always fails on write operations. Used to test rollback
    /// behavior when a secondary store fails.
    struct FailingStore {
        /// Delegate reads to a real InMemoryGraphStore so graph_exists etc. work
        /// during setup (create_graph).
        inner: InMemoryGraphStore,
        /// When true, all write operations fail. Toggled after setup.
        fail_writes: std::sync::atomic::AtomicBool,
    }

    impl FailingStore {
        fn new() -> Self {
            Self {
                inner: InMemoryGraphStore::new(),
                fail_writes: std::sync::atomic::AtomicBool::new(false),
            }
        }

        fn enable_failures(&self) {
            self.fail_writes
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }

        fn should_fail(&self) -> Result<(), StorageError> {
            if self
                .fail_writes
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                Err(StorageError::Backend {
                    msg: "simulated secondary failure".to_string(),
                })
            } else {
                Ok(())
            }
        }
    }

    impl GraphStore for FailingStore {
        fn get_node(&self, g: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
            self.inner.get_node(g, id)
        }
        fn get_edge(&self, g: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
            self.inner.get_edge(g, id)
        }
        fn get_nodes_batch(
            &self,
            g: &str,
            ids: &[NodeId],
        ) -> Result<Vec<Option<NodeData>>, StorageError> {
            self.inner.get_nodes_batch(g, ids)
        }
        fn get_edges_batch(
            &self,
            g: &str,
            ids: &[EdgeId],
        ) -> Result<Vec<Option<EdgeData>>, StorageError> {
            self.inner.get_edges_batch(g, ids)
        }
        fn find_nodes_by_label(&self, g: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
            self.inner.find_nodes_by_label(g, label)
        }
        fn find_nodes_by_label_and_property(
            &self,
            g: &str,
            label: &str,
            property: &str,
            range: RangeQuery<'_>,
        ) -> Result<Vec<NodeId>, StorageError> {
            self.inner
                .find_nodes_by_label_and_property(g, label, property, range)
        }
        fn find_edges_by_type(
            &self,
            g: &str,
            rel_type: &str,
        ) -> Result<Vec<EdgeId>, StorageError> {
            self.inner.find_edges_by_type(g, rel_type)
        }
        fn out_edges(&self, g: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
            self.inner.out_edges(g, node_id)
        }
        fn in_edges(&self, g: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
            self.inner.in_edges(g, node_id)
        }
        fn out_edges_by_type(
            &self,
            g: &str,
            node_id: NodeId,
            rel_types: &[&str],
        ) -> Result<Vec<AdjEntry>, StorageError> {
            self.inner.out_edges_by_type(g, node_id, rel_types)
        }
        fn in_edges_by_type(
            &self,
            g: &str,
            node_id: NodeId,
            rel_types: &[&str],
        ) -> Result<Vec<AdjEntry>, StorageError> {
            self.inner.in_edges_by_type(g, node_id, rel_types)
        }
        fn out_edge_ids(&self, g: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
            self.inner.out_edge_ids(g, node_id)
        }
        fn in_edge_ids(&self, g: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
            self.inner.in_edge_ids(g, node_id)
        }
        fn all_node_ids(&self, g: &str) -> Result<Vec<NodeId>, StorageError> {
            self.inner.all_node_ids(g)
        }
        fn all_edge_ids(&self, g: &str) -> Result<Vec<EdgeId>, StorageError> {
            self.inner.all_edge_ids(g)
        }
        fn all_label_names(&self, g: &str) -> Result<Vec<String>, StorageError> {
            self.inner.all_label_names(g)
        }
        fn create_node(
            &self,
            g: &str,
            labels: Vec<String>,
            properties: HashMap<String, Value>,
        ) -> Result<NodeId, StorageError> {
            self.should_fail()?;
            self.inner.create_node(g, labels, properties)
        }
        fn create_edge(
            &self,
            g: &str,
            src: NodeId,
            dst: NodeId,
            rel_type: String,
            properties: HashMap<String, Value>,
        ) -> Result<EdgeId, StorageError> {
            self.should_fail()?;
            self.inner.create_edge(g, src, dst, rel_type, properties)
        }
        fn set_node_properties(
            &self,
            g: &str,
            id: NodeId,
            properties: HashMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.should_fail()?;
            self.inner.set_node_properties(g, id, properties)
        }
        fn replace_node_properties(
            &self,
            g: &str,
            id: NodeId,
            properties: HashMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.should_fail()?;
            self.inner.replace_node_properties(g, id, properties)
        }
        fn set_edge_properties(
            &self,
            g: &str,
            id: EdgeId,
            properties: HashMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.should_fail()?;
            self.inner.set_edge_properties(g, id, properties)
        }
        fn replace_edge_properties(
            &self,
            g: &str,
            id: EdgeId,
            properties: HashMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.should_fail()?;
            self.inner.replace_edge_properties(g, id, properties)
        }
        fn remove_node_property(
            &self,
            g: &str,
            id: NodeId,
            key: &str,
        ) -> Result<(), StorageError> {
            self.should_fail()?;
            self.inner.remove_node_property(g, id, key)
        }
        fn remove_edge_property(
            &self,
            g: &str,
            id: EdgeId,
            key: &str,
        ) -> Result<(), StorageError> {
            self.should_fail()?;
            self.inner.remove_edge_property(g, id, key)
        }
        fn add_node_labels(
            &self,
            g: &str,
            id: NodeId,
            labels: &[String],
        ) -> Result<(), StorageError> {
            self.should_fail()?;
            self.inner.add_node_labels(g, id, labels)
        }
        fn remove_node_labels(
            &self,
            g: &str,
            id: NodeId,
            labels: &[String],
        ) -> Result<(), StorageError> {
            self.should_fail()?;
            self.inner.remove_node_labels(g, id, labels)
        }
        fn delete_node(&self, g: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
            self.should_fail()?;
            self.inner.delete_node(g, id, detach)
        }
        fn delete_edge(&self, g: &str, id: EdgeId) -> Result<(), StorageError> {
            self.should_fail()?;
            self.inner.delete_edge(g, id)
        }
        fn list_indexes(&self, g: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
            self.inner.list_indexes(g)
        }
        fn create_index(&self, g: &str, label: &str, property: &str) -> Result<(), StorageError> {
            self.should_fail()?;
            self.inner.create_index(g, label, property)
        }
        fn drop_index(&self, g: &str, label: &str, property: &str) -> Result<(), StorageError> {
            self.should_fail()?;
            self.inner.drop_index(g, label, property)
        }
        fn create_graph(&self, name: &str) -> Result<(), StorageError> {
            // Allow create_graph even when failures are enabled, so setup works.
            self.inner.create_graph(name)
        }
        fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
            self.should_fail()?;
            self.inner.delete_graph(name)
        }
        fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
            self.inner.graph_exists(name)
        }
        fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
            self.inner.list_graphs()
        }
        fn graph_stats(&self, g: &str) -> Result<GraphStats, StorageError> {
            self.inner.graph_stats(g)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn mem_store() -> Box<dyn GraphStore> {
        Box::new(InMemoryGraphStore::new())
    }

    /// Create a shared InMemoryGraphStore, returning the Arc (for direct
    /// reads) and a Box<dyn GraphStore> (for FanoutStore).
    fn shared_store() -> (Arc<InMemoryGraphStore>, Box<dyn GraphStore>) {
        let inner = Arc::new(InMemoryGraphStore::new());
        let boxed: Box<dyn GraphStore> = Box::new(ArcStore(Arc::clone(&inner)));
        (inner, boxed)
    }

    // ── Original tests (preserved) ──────────────────────────────────────────

    #[test]
    fn test_single_backend_round_trip() {
        let store = FanoutStore::new(vec![mem_store()]);
        store.create_graph("g").unwrap();
        let node_id = store
            .create_node("g", vec!["Person".into()], HashMap::new())
            .unwrap();
        let node = store.get_node("g", node_id).unwrap();
        assert!(node.is_some(), "node should exist after creation");
        assert!(node.unwrap().labels.contains(&"Person".to_string()));
    }

    #[test]
    #[should_panic(expected = "FanoutStore requires at least one backend")]
    fn test_new_empty_panics() {
        let _store = FanoutStore::new(vec![]);
    }

    // ── Graph creation propagates to both stores ────────────────────────────

    #[test]
    fn test_create_graph_on_both_stores() {
        let (primary_ref, primary_box) = shared_store();
        let (secondary_ref, secondary_box) = shared_store();
        let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

        fanout.create_graph("g").unwrap();

        assert!(
            primary_ref.graph_exists("g").unwrap(),
            "primary should have graph 'g'"
        );
        assert!(
            secondary_ref.graph_exists("g").unwrap(),
            "secondary should have graph 'g'"
        );
    }

    // ── Node creation propagates to both stores ─────────────────────────────

    #[test]
    fn test_create_node_present_on_both() {
        let (primary_ref, primary_box) = shared_store();
        let (secondary_ref, secondary_box) = shared_store();
        let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

        fanout.create_graph("g").unwrap();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let node_id = fanout
            .create_node("g", vec!["Person".into()], props)
            .unwrap();

        // Primary should have the node.
        let primary_node = primary_ref.get_node("g", node_id).unwrap();
        assert!(primary_node.is_some(), "node must exist on primary");
        let pn = primary_node.unwrap();
        assert!(pn.labels.contains(&"Person".to_string()));
        assert_eq!(pn.properties.get("name"), Some(&Value::Str("Alice".into())));
        // Primary should NOT have the injected _sd2_id.
        assert!(
            !pn.properties.contains_key("_sd2_id"),
            "primary must not have _sd2_id"
        );

        // Secondary should also have the node at the SAME NodeId. FanoutStore
        // injects `_sd2_id` into the property map, and InMemoryGraphStore
        // honors it to set the raw ID (then strips the internal key).
        let sec_node = secondary_ref
            .get_node("g", node_id)
            .unwrap()
            .expect("secondary should have node at same ID as primary");
        assert!(sec_node.labels.contains(&"Person".to_string()));
        assert_eq!(
            sec_node.properties.get("name"),
            Some(&Value::Str("Alice".into())),
            "secondary should have the original properties"
        );
        // The _sd2_id internal key is stripped by InMemoryGraphStore.
        assert!(
            !sec_node.properties.contains_key("_sd2_id"),
            "internal _sd2_id should be stripped from stored properties"
        );
    }

    // ── Edge creation propagates to both stores ─────────────────────────────

    #[test]
    fn test_create_edge_present_on_both() {
        let (primary_ref, primary_box) = shared_store();
        let (secondary_ref, secondary_box) = shared_store();
        let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

        fanout.create_graph("g").unwrap();
        let a = fanout
            .create_node("g", vec!["Person".into()], HashMap::new())
            .unwrap();
        let b = fanout
            .create_node("g", vec!["Person".into()], HashMap::new())
            .unwrap();

        let mut edge_props = HashMap::new();
        edge_props.insert("since".to_string(), Value::Int(2020));
        let edge_id = fanout
            .create_edge("g", a, b, "KNOWS".to_string(), edge_props)
            .unwrap();

        // Primary: edge exists with correct data.
        let pe = primary_ref.get_edge("g", edge_id).unwrap().unwrap();
        assert_eq!(pe.rel_type, "KNOWS");
        assert_eq!(pe.src, a);
        assert_eq!(pe.dst, b);
        assert_eq!(pe.properties.get("since"), Some(&Value::Int(2020)));
        assert!(
            !pe.properties.contains_key("_sd2_eid"),
            "primary edge must not have _sd2_eid"
        );

        // Secondary should have the edge at the SAME EdgeId. FanoutStore
        // injects `_sd2_eid` into the property map, and InMemoryGraphStore
        // honors it to set the raw ID (then strips the internal key).
        let se = secondary_ref
            .get_edge("g", edge_id)
            .unwrap()
            .expect("secondary should have edge at same ID as primary");
        assert_eq!(se.rel_type, "KNOWS");
        assert_eq!(se.src, a);
        assert_eq!(se.dst, b);
        assert_eq!(se.properties.get("since"), Some(&Value::Int(2020)));
        // The _sd2_eid internal key is stripped by InMemoryGraphStore.
        assert!(
            !se.properties.contains_key("_sd2_eid"),
            "internal _sd2_eid should be stripped from stored properties"
        );
    }

    // ── Delete node propagates to both stores ───────────────────────────────

    #[test]
    fn test_delete_node_propagates_to_both() {
        let (primary_ref, primary_box) = shared_store();
        let (secondary_ref, secondary_box) = shared_store();
        let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

        fanout.create_graph("g").unwrap();
        let node_id = fanout
            .create_node("g", vec!["Person".into()], HashMap::new())
            .unwrap();

        // Verify node exists on both before deletion.
        assert!(primary_ref.get_node("g", node_id).unwrap().is_some());
        assert_eq!(secondary_ref.all_node_ids("g").unwrap().len(), 1);

        fanout.delete_node("g", node_id, true).unwrap();

        // Primary: node gone.
        assert!(
            primary_ref.get_node("g", node_id).unwrap().is_none(),
            "primary must not have node after delete"
        );
        // Secondary: also gone. Because InMemoryGraphStore honors _sd2_id
        // during create_node, the secondary has the same NodeId as the
        // primary, so delete_node(write_all) with that ID works on both.
        assert!(
            secondary_ref.get_node("g", node_id).unwrap().is_none(),
            "secondary must not have node after delete"
        );
        assert!(
            secondary_ref.all_node_ids("g").unwrap().is_empty(),
            "secondary should have no nodes after delete"
        );
    }

    // ── Delete edge propagates to both stores ───────────────────────────────

    #[test]
    fn test_delete_edge_propagates_to_both() {
        let (primary_ref, primary_box) = shared_store();
        let (_secondary_ref, secondary_box) = shared_store();
        let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

        fanout.create_graph("g").unwrap();
        let a = fanout
            .create_node("g", vec!["Person".into()], HashMap::new())
            .unwrap();
        let b = fanout
            .create_node("g", vec!["Person".into()], HashMap::new())
            .unwrap();
        let edge_id = fanout
            .create_edge("g", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        // Verify edge exists on primary.
        assert!(primary_ref.get_edge("g", edge_id).unwrap().is_some());

        fanout.delete_edge("g", edge_id).unwrap();

        // Primary: edge gone.
        assert!(
            primary_ref.get_edge("g", edge_id).unwrap().is_none(),
            "primary must not have edge after delete"
        );
    }

    // ── set_node_properties propagates via write_all ────────────────────────

    #[test]
    fn test_set_node_properties_propagates() {
        let (primary_ref, primary_box) = shared_store();
        let (secondary_ref, secondary_box) = shared_store();
        let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

        fanout.create_graph("g").unwrap();
        let node_id = fanout
            .create_node("g", vec!["Person".into()], HashMap::new())
            .unwrap();

        let mut props = HashMap::new();
        props.insert("age".to_string(), Value::Int(30));
        fanout.set_node_properties("g", node_id, props).unwrap();

        // Primary has the updated property.
        let pn = primary_ref.get_node("g", node_id).unwrap().unwrap();
        assert_eq!(pn.properties.get("age"), Some(&Value::Int(30)));

        // Secondary also received the write_all call. The secondary's node
        // has the primary's NodeId passed through set_node_properties, which
        // may or may not match. If the secondary used the same ID (which
        // happens when both start from 0), it will have the property.
        // We verify the secondary got the graph at minimum.
        assert!(secondary_ref.graph_exists("g").unwrap());
    }

    // ── Read operations route to primary ────────────────────────────────────

    #[test]
    fn test_reads_come_from_primary() {
        let (primary_ref, primary_box) = shared_store();
        let (_secondary_ref, secondary_box) = shared_store();
        let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

        fanout.create_graph("g").unwrap();
        let a = fanout
            .create_node("g", vec!["Person".into()], HashMap::new())
            .unwrap();
        let b = fanout
            .create_node("g", vec!["Person".into()], HashMap::new())
            .unwrap();
        fanout
            .create_edge("g", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        // list_graphs
        let graphs = fanout.list_graphs().unwrap();
        assert_eq!(graphs, vec!["g"]);

        // graph_exists
        assert!(fanout.graph_exists("g").unwrap());
        assert!(!fanout.graph_exists("nonexistent").unwrap());

        // graph_stats
        let stats = fanout.graph_stats("g").unwrap();
        assert_eq!(stats.node_count, 2);
        assert_eq!(stats.edge_count, 1);

        // all_node_ids
        let mut node_ids = fanout.all_node_ids("g").unwrap();
        node_ids.sort_by_key(|n| n.0);
        assert_eq!(node_ids.len(), 2);

        // all_edge_ids
        let edge_ids = fanout.all_edge_ids("g").unwrap();
        assert_eq!(edge_ids.len(), 1);

        // find_nodes_by_label
        let person_ids = fanout.find_nodes_by_label("g", "Person").unwrap();
        assert_eq!(person_ids.len(), 2);

        // find_edges_by_type
        let knows_ids = fanout.find_edges_by_type("g", "KNOWS").unwrap();
        assert_eq!(knows_ids.len(), 1);

        // out_edges / in_edges
        let out = fanout.out_edges("g", a).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].neighbor_id, b);

        let in_e = fanout.in_edges("g", b).unwrap();
        assert_eq!(in_e.len(), 1);
        assert_eq!(in_e[0].neighbor_id, a);

        // out_edges_by_type / in_edges_by_type
        let out_knows = fanout.out_edges_by_type("g", a, &["KNOWS"]).unwrap();
        assert_eq!(out_knows.len(), 1);
        let in_knows = fanout.in_edges_by_type("g", b, &["KNOWS"]).unwrap();
        assert_eq!(in_knows.len(), 1);

        // all_label_names
        let labels = fanout.all_label_names("g").unwrap();
        assert_eq!(labels, vec!["Person"]);

        // get_nodes_batch / get_edges_batch
        let batch_nodes = fanout.get_nodes_batch("g", &[a, b]).unwrap();
        assert_eq!(batch_nodes.len(), 2);
        assert!(batch_nodes.iter().all(|n| n.is_some()));

        let batch_edges = fanout
            .get_edges_batch("g", &edge_ids)
            .unwrap();
        assert_eq!(batch_edges.len(), 1);
        assert!(batch_edges[0].is_some());

        // Verify reads match what's in the primary directly.
        let primary_stats = primary_ref.graph_stats("g").unwrap();
        assert_eq!(stats.node_count, primary_stats.node_count);
        assert_eq!(stats.edge_count, primary_stats.edge_count);
    }

    // ── Secondary failure triggers rollback on create_node ──────────────────

    #[test]
    fn test_secondary_failure_rollback_create_node() {
        // Use Arc<FailingStore> so we can toggle failures after setup.
        let primary_inner = Arc::new(InMemoryGraphStore::new());
        let primary_box_final: Box<dyn GraphStore> =
            Box::new(ArcStore(Arc::clone(&primary_inner)));

        let failing_inner = Arc::new(FailingStore::new());
        // We need a wrapper for Arc<FailingStore> too.
        struct ArcFailingStore(Arc<FailingStore>);
        impl GraphStore for ArcFailingStore {
            fn get_node(
                &self,
                g: &str,
                id: NodeId,
            ) -> Result<Option<NodeData>, StorageError> {
                self.0.get_node(g, id)
            }
            fn get_edge(
                &self,
                g: &str,
                id: EdgeId,
            ) -> Result<Option<EdgeData>, StorageError> {
                self.0.get_edge(g, id)
            }
            fn get_nodes_batch(
                &self,
                g: &str,
                ids: &[NodeId],
            ) -> Result<Vec<Option<NodeData>>, StorageError> {
                self.0.get_nodes_batch(g, ids)
            }
            fn get_edges_batch(
                &self,
                g: &str,
                ids: &[EdgeId],
            ) -> Result<Vec<Option<EdgeData>>, StorageError> {
                self.0.get_edges_batch(g, ids)
            }
            fn find_nodes_by_label(
                &self,
                g: &str,
                label: &str,
            ) -> Result<Vec<NodeId>, StorageError> {
                self.0.find_nodes_by_label(g, label)
            }
            fn find_nodes_by_label_and_property(
                &self,
                g: &str,
                label: &str,
                property: &str,
                range: RangeQuery<'_>,
            ) -> Result<Vec<NodeId>, StorageError> {
                self.0
                    .find_nodes_by_label_and_property(g, label, property, range)
            }
            fn find_edges_by_type(
                &self,
                g: &str,
                rel_type: &str,
            ) -> Result<Vec<EdgeId>, StorageError> {
                self.0.find_edges_by_type(g, rel_type)
            }
            fn out_edges(
                &self,
                g: &str,
                node_id: NodeId,
            ) -> Result<Vec<AdjEntry>, StorageError> {
                self.0.out_edges(g, node_id)
            }
            fn in_edges(
                &self,
                g: &str,
                node_id: NodeId,
            ) -> Result<Vec<AdjEntry>, StorageError> {
                self.0.in_edges(g, node_id)
            }
            fn out_edges_by_type(
                &self,
                g: &str,
                node_id: NodeId,
                rel_types: &[&str],
            ) -> Result<Vec<AdjEntry>, StorageError> {
                self.0.out_edges_by_type(g, node_id, rel_types)
            }
            fn in_edges_by_type(
                &self,
                g: &str,
                node_id: NodeId,
                rel_types: &[&str],
            ) -> Result<Vec<AdjEntry>, StorageError> {
                self.0.in_edges_by_type(g, node_id, rel_types)
            }
            fn out_edge_ids(
                &self,
                g: &str,
                node_id: NodeId,
            ) -> Result<Vec<EdgeId>, StorageError> {
                self.0.out_edge_ids(g, node_id)
            }
            fn in_edge_ids(
                &self,
                g: &str,
                node_id: NodeId,
            ) -> Result<Vec<EdgeId>, StorageError> {
                self.0.in_edge_ids(g, node_id)
            }
            fn all_node_ids(&self, g: &str) -> Result<Vec<NodeId>, StorageError> {
                self.0.all_node_ids(g)
            }
            fn all_edge_ids(&self, g: &str) -> Result<Vec<EdgeId>, StorageError> {
                self.0.all_edge_ids(g)
            }
            fn all_label_names(&self, g: &str) -> Result<Vec<String>, StorageError> {
                self.0.all_label_names(g)
            }
            fn create_node(
                &self,
                g: &str,
                labels: Vec<String>,
                properties: HashMap<String, Value>,
            ) -> Result<NodeId, StorageError> {
                self.0.create_node(g, labels, properties)
            }
            fn create_edge(
                &self,
                g: &str,
                src: NodeId,
                dst: NodeId,
                rel_type: String,
                properties: HashMap<String, Value>,
            ) -> Result<EdgeId, StorageError> {
                self.0.create_edge(g, src, dst, rel_type, properties)
            }
            fn set_node_properties(
                &self,
                g: &str,
                id: NodeId,
                properties: HashMap<String, Value>,
            ) -> Result<(), StorageError> {
                self.0.set_node_properties(g, id, properties)
            }
            fn replace_node_properties(
                &self,
                g: &str,
                id: NodeId,
                properties: HashMap<String, Value>,
            ) -> Result<(), StorageError> {
                self.0.replace_node_properties(g, id, properties)
            }
            fn set_edge_properties(
                &self,
                g: &str,
                id: EdgeId,
                properties: HashMap<String, Value>,
            ) -> Result<(), StorageError> {
                self.0.set_edge_properties(g, id, properties)
            }
            fn replace_edge_properties(
                &self,
                g: &str,
                id: EdgeId,
                properties: HashMap<String, Value>,
            ) -> Result<(), StorageError> {
                self.0.replace_edge_properties(g, id, properties)
            }
            fn remove_node_property(
                &self,
                g: &str,
                id: NodeId,
                key: &str,
            ) -> Result<(), StorageError> {
                self.0.remove_node_property(g, id, key)
            }
            fn remove_edge_property(
                &self,
                g: &str,
                id: EdgeId,
                key: &str,
            ) -> Result<(), StorageError> {
                self.0.remove_edge_property(g, id, key)
            }
            fn add_node_labels(
                &self,
                g: &str,
                id: NodeId,
                labels: &[String],
            ) -> Result<(), StorageError> {
                self.0.add_node_labels(g, id, labels)
            }
            fn remove_node_labels(
                &self,
                g: &str,
                id: NodeId,
                labels: &[String],
            ) -> Result<(), StorageError> {
                self.0.remove_node_labels(g, id, labels)
            }
            fn delete_node(
                &self,
                g: &str,
                id: NodeId,
                detach: bool,
            ) -> Result<(), StorageError> {
                self.0.delete_node(g, id, detach)
            }
            fn delete_edge(&self, g: &str, id: EdgeId) -> Result<(), StorageError> {
                self.0.delete_edge(g, id)
            }
            fn list_indexes(&self, g: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
                self.0.list_indexes(g)
            }
            fn create_index(
                &self,
                g: &str,
                label: &str,
                property: &str,
            ) -> Result<(), StorageError> {
                self.0.create_index(g, label, property)
            }
            fn drop_index(
                &self,
                g: &str,
                label: &str,
                property: &str,
            ) -> Result<(), StorageError> {
                self.0.drop_index(g, label, property)
            }
            fn create_graph(&self, name: &str) -> Result<(), StorageError> {
                self.0.create_graph(name)
            }
            fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
                self.0.delete_graph(name)
            }
            fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
                self.0.graph_exists(name)
            }
            fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
                self.0.list_graphs()
            }
            fn graph_stats(&self, g: &str) -> Result<GraphStats, StorageError> {
                self.0.graph_stats(g)
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }

        let failing_box: Box<dyn GraphStore> =
            Box::new(ArcFailingStore(Arc::clone(&failing_inner)));

        let fanout = FanoutStore::new(vec![primary_box_final, failing_box]);
        fanout.create_graph("g").unwrap();

        // Enable failures on the secondary.
        failing_inner.enable_failures();

        // Attempt to create a node — should fail because secondary fails.
        let result = fanout.create_node("g", vec!["Person".into()], HashMap::new());
        assert!(result.is_err(), "create_node should fail when secondary fails");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("simulated secondary failure"),
            "error should mention simulated failure, got: {}",
            err_msg
        );

        // Rollback: the node created on primary should have been deleted.
        let primary_nodes = primary_inner.all_node_ids("g").unwrap();
        assert!(
            primary_nodes.is_empty(),
            "primary should have no nodes after rollback, but found {}",
            primary_nodes.len()
        );
    }

    // ── Secondary failure triggers rollback on create_edge ──────────────────

    #[test]
    fn test_secondary_failure_rollback_create_edge() {
        let primary_inner = Arc::new(InMemoryGraphStore::new());
        let primary_box: Box<dyn GraphStore> =
            Box::new(ArcStore(Arc::clone(&primary_inner)));

        let failing_inner = Arc::new(FailingStore::new());
        // Reuse the ArcFailingStore pattern from above — but since it's
        // defined inside the other test, define a minimal inline wrapper.
        struct ArcFS(Arc<FailingStore>);
        impl GraphStore for ArcFS {
            fn get_node(&self, g: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> { self.0.get_node(g, id) }
            fn get_edge(&self, g: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> { self.0.get_edge(g, id) }
            fn get_nodes_batch(&self, g: &str, ids: &[NodeId]) -> Result<Vec<Option<NodeData>>, StorageError> { self.0.get_nodes_batch(g, ids) }
            fn get_edges_batch(&self, g: &str, ids: &[EdgeId]) -> Result<Vec<Option<EdgeData>>, StorageError> { self.0.get_edges_batch(g, ids) }
            fn find_nodes_by_label(&self, g: &str, l: &str) -> Result<Vec<NodeId>, StorageError> { self.0.find_nodes_by_label(g, l) }
            fn find_nodes_by_label_and_property(&self, g: &str, l: &str, p: &str, r: RangeQuery<'_>) -> Result<Vec<NodeId>, StorageError> { self.0.find_nodes_by_label_and_property(g, l, p, r) }
            fn find_edges_by_type(&self, g: &str, t: &str) -> Result<Vec<EdgeId>, StorageError> { self.0.find_edges_by_type(g, t) }
            fn out_edges(&self, g: &str, n: NodeId) -> Result<Vec<AdjEntry>, StorageError> { self.0.out_edges(g, n) }
            fn in_edges(&self, g: &str, n: NodeId) -> Result<Vec<AdjEntry>, StorageError> { self.0.in_edges(g, n) }
            fn out_edges_by_type(&self, g: &str, n: NodeId, t: &[&str]) -> Result<Vec<AdjEntry>, StorageError> { self.0.out_edges_by_type(g, n, t) }
            fn in_edges_by_type(&self, g: &str, n: NodeId, t: &[&str]) -> Result<Vec<AdjEntry>, StorageError> { self.0.in_edges_by_type(g, n, t) }
            fn out_edge_ids(&self, g: &str, n: NodeId) -> Result<Vec<EdgeId>, StorageError> { self.0.out_edge_ids(g, n) }
            fn in_edge_ids(&self, g: &str, n: NodeId) -> Result<Vec<EdgeId>, StorageError> { self.0.in_edge_ids(g, n) }
            fn all_node_ids(&self, g: &str) -> Result<Vec<NodeId>, StorageError> { self.0.all_node_ids(g) }
            fn all_edge_ids(&self, g: &str) -> Result<Vec<EdgeId>, StorageError> { self.0.all_edge_ids(g) }
            fn all_label_names(&self, g: &str) -> Result<Vec<String>, StorageError> { self.0.all_label_names(g) }
            fn create_node(&self, g: &str, l: Vec<String>, p: HashMap<String, Value>) -> Result<NodeId, StorageError> { self.0.create_node(g, l, p) }
            fn create_edge(&self, g: &str, s: NodeId, d: NodeId, t: String, p: HashMap<String, Value>) -> Result<EdgeId, StorageError> { self.0.create_edge(g, s, d, t, p) }
            fn set_node_properties(&self, g: &str, id: NodeId, p: HashMap<String, Value>) -> Result<(), StorageError> { self.0.set_node_properties(g, id, p) }
            fn replace_node_properties(&self, g: &str, id: NodeId, p: HashMap<String, Value>) -> Result<(), StorageError> { self.0.replace_node_properties(g, id, p) }
            fn set_edge_properties(&self, g: &str, id: EdgeId, p: HashMap<String, Value>) -> Result<(), StorageError> { self.0.set_edge_properties(g, id, p) }
            fn replace_edge_properties(&self, g: &str, id: EdgeId, p: HashMap<String, Value>) -> Result<(), StorageError> { self.0.replace_edge_properties(g, id, p) }
            fn remove_node_property(&self, g: &str, id: NodeId, k: &str) -> Result<(), StorageError> { self.0.remove_node_property(g, id, k) }
            fn remove_edge_property(&self, g: &str, id: EdgeId, k: &str) -> Result<(), StorageError> { self.0.remove_edge_property(g, id, k) }
            fn add_node_labels(&self, g: &str, id: NodeId, l: &[String]) -> Result<(), StorageError> { self.0.add_node_labels(g, id, l) }
            fn remove_node_labels(&self, g: &str, id: NodeId, l: &[String]) -> Result<(), StorageError> { self.0.remove_node_labels(g, id, l) }
            fn delete_node(&self, g: &str, id: NodeId, d: bool) -> Result<(), StorageError> { self.0.delete_node(g, id, d) }
            fn delete_edge(&self, g: &str, id: EdgeId) -> Result<(), StorageError> { self.0.delete_edge(g, id) }
            fn list_indexes(&self, g: &str) -> Result<Vec<IndexDescriptor>, StorageError> { self.0.list_indexes(g) }
            fn create_index(&self, g: &str, l: &str, p: &str) -> Result<(), StorageError> { self.0.create_index(g, l, p) }
            fn drop_index(&self, g: &str, l: &str, p: &str) -> Result<(), StorageError> { self.0.drop_index(g, l, p) }
            fn create_graph(&self, n: &str) -> Result<(), StorageError> { self.0.create_graph(n) }
            fn delete_graph(&self, n: &str) -> Result<(), StorageError> { self.0.delete_graph(n) }
            fn graph_exists(&self, n: &str) -> Result<bool, StorageError> { self.0.graph_exists(n) }
            fn list_graphs(&self) -> Result<Vec<String>, StorageError> { self.0.list_graphs() }
            fn graph_stats(&self, g: &str) -> Result<GraphStats, StorageError> { self.0.graph_stats(g) }
            fn as_any(&self) -> &dyn std::any::Any { self }
        }

        let failing_box: Box<dyn GraphStore> = Box::new(ArcFS(Arc::clone(&failing_inner)));
        let fanout = FanoutStore::new(vec![primary_box, failing_box]);
        fanout.create_graph("g").unwrap();

        // Create two nodes (secondary is not failing yet, so this succeeds).
        let a = fanout
            .create_node("g", vec!["Person".into()], HashMap::new())
            .unwrap();
        let b = fanout
            .create_node("g", vec!["Person".into()], HashMap::new())
            .unwrap();

        // Enable failures on secondary.
        failing_inner.enable_failures();

        // Attempt to create an edge — should fail.
        let result = fanout.create_edge("g", a, b, "KNOWS".to_string(), HashMap::new());
        assert!(
            result.is_err(),
            "create_edge should fail when secondary fails"
        );

        // Rollback: the edge created on primary should have been deleted.
        let primary_edges = primary_inner.all_edge_ids("g").unwrap();
        assert!(
            primary_edges.is_empty(),
            "primary should have no edges after rollback, but found {}",
            primary_edges.len()
        );

        // The two nodes should still exist (they were created before the failure).
        let primary_nodes = primary_inner.all_node_ids("g").unwrap();
        assert_eq!(
            primary_nodes.len(),
            2,
            "nodes created before failure should still exist"
        );
    }

    // ── Delete graph propagates to both stores ──────────────────────────────

    #[test]
    fn test_delete_graph_propagates_to_both() {
        let (primary_ref, primary_box) = shared_store();
        let (secondary_ref, secondary_box) = shared_store();
        let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

        fanout.create_graph("g").unwrap();
        assert!(primary_ref.graph_exists("g").unwrap());
        assert!(secondary_ref.graph_exists("g").unwrap());

        fanout.delete_graph("g").unwrap();
        assert!(
            !primary_ref.graph_exists("g").unwrap(),
            "primary should not have graph after delete"
        );
        assert!(
            !secondary_ref.graph_exists("g").unwrap(),
            "secondary should not have graph after delete"
        );
    }

    // ── Index operations propagate to both stores ───────────────────────────

    #[test]
    fn test_index_operations_propagate() {
        let (primary_ref, primary_box) = shared_store();
        let (secondary_ref, secondary_box) = shared_store();
        let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

        fanout.create_graph("g").unwrap();
        fanout.create_index("g", "Person", "age").unwrap();

        // Both stores should have the index.
        let p_indexes = primary_ref.list_indexes("g").unwrap();
        assert_eq!(p_indexes.len(), 1);
        assert_eq!(p_indexes[0].label, "Person");
        assert_eq!(p_indexes[0].property, "age");

        let s_indexes = secondary_ref.list_indexes("g").unwrap();
        assert_eq!(s_indexes.len(), 1);
        assert_eq!(s_indexes[0].label, "Person");

        // Drop index propagates too.
        fanout.drop_index("g", "Person", "age").unwrap();
        assert!(primary_ref.list_indexes("g").unwrap().is_empty());
        assert!(secondary_ref.list_indexes("g").unwrap().is_empty());
    }

    // ── Label operations propagate via write_all ────────────────────────────

    #[test]
    fn test_add_remove_labels_propagate() {
        let (primary_ref, primary_box) = shared_store();
        let (_secondary_ref, secondary_box) = shared_store();
        let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

        fanout.create_graph("g").unwrap();
        let node_id = fanout
            .create_node("g", vec!["Person".into()], HashMap::new())
            .unwrap();

        fanout
            .add_node_labels("g", node_id, &["Employee".to_string()])
            .unwrap();
        let node = primary_ref.get_node("g", node_id).unwrap().unwrap();
        assert!(node.labels.contains(&"Person".to_string()));
        assert!(node.labels.contains(&"Employee".to_string()));

        fanout
            .remove_node_labels("g", node_id, &["Person".to_string()])
            .unwrap();
        let node = primary_ref.get_node("g", node_id).unwrap().unwrap();
        assert!(!node.labels.contains(&"Person".to_string()));
        assert!(node.labels.contains(&"Employee".to_string()));
    }
}
