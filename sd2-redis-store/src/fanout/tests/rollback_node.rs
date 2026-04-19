// Tests for FanoutStore — rollback_node behavior.

use super::*;

#[test]
fn test_secondary_failure_rollback_create_node() {
    // Use Arc<FailingStore> so we can toggle failures after setup.
    let primary_inner = Arc::new(InMemoryGraphStore::new());
    let primary_box_final: Box<dyn GraphStore> = Box::new(ArcStore(Arc::clone(&primary_inner)));

    let failing_inner = Arc::new(FailingStore::new());
    // We need a wrapper for Arc<FailingStore> too.
    struct ArcFailingStore(Arc<FailingStore>);
    impl GraphStore for ArcFailingStore {
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
        fn find_edges_by_type(&self, g: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
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
            properties: IndexMap<String, Value>,
        ) -> Result<NodeId, StorageError> {
            self.0.create_node(g, labels, properties)
        }
        fn create_edge(
            &self,
            g: &str,
            src: NodeId,
            dst: NodeId,
            rel_type: String,
            properties: IndexMap<String, Value>,
        ) -> Result<EdgeId, StorageError> {
            self.0.create_edge(g, src, dst, rel_type, properties)
        }
        fn set_node_properties(
            &self,
            g: &str,
            id: NodeId,
            properties: IndexMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.0.set_node_properties(g, id, properties)
        }
        fn replace_node_properties(
            &self,
            g: &str,
            id: NodeId,
            properties: IndexMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.0.replace_node_properties(g, id, properties)
        }
        fn set_edge_properties(
            &self,
            g: &str,
            id: EdgeId,
            properties: IndexMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.0.set_edge_properties(g, id, properties)
        }
        fn replace_edge_properties(
            &self,
            g: &str,
            id: EdgeId,
            properties: IndexMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.0.replace_edge_properties(g, id, properties)
        }
        fn remove_node_property(&self, g: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
            self.0.remove_node_property(g, id, key)
        }
        fn remove_edge_property(&self, g: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
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

    let failing_box: Box<dyn GraphStore> = Box::new(ArcFailingStore(Arc::clone(&failing_inner)));

    let fanout = FanoutStore::new(vec![primary_box_final, failing_box]);
    fanout.create_graph("g").unwrap();

    // Enable failures on the secondary.
    failing_inner.enable_failures();

    // Attempt to create a node — should fail because secondary fails.
    let result = fanout.create_node("g", vec!["Person".into()], IndexMap::new());
    assert!(
        result.is_err(),
        "create_node should fail when secondary fails"
    );
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
fn test_late_secondary_failure_rollback_create_node_cleans_earlier_secondary() {
    let (primary_ref, primary_box) = shared_store();
    let (middle_ref, middle_box) = shared_store();
    let (late_ref, late_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, middle_box, late_box]);

    fanout.create_graph("g").unwrap();
    late_ref.delete_graph("g").unwrap();

    let result = fanout.create_node("g", vec!["Person".into()], IndexMap::new());
    assert!(
        result.is_err(),
        "create_node should fail when a later secondary is out of sync"
    );

    assert!(
        primary_ref.all_node_ids("g").unwrap().is_empty(),
        "primary should be rolled back after the late secondary failure"
    );
    assert!(
        middle_ref.all_node_ids("g").unwrap().is_empty(),
        "earlier successful secondaries should also be rolled back"
    );
    assert!(
        !late_ref.graph_exists("g").unwrap(),
        "the failing late secondary should remain in its pre-failure state"
    );
}
