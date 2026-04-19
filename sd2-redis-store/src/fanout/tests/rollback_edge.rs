// Tests for FanoutStore — rollback_edge behavior.

use super::*;

#[test]
fn test_secondary_failure_rollback_create_edge() {
    let primary_inner = Arc::new(InMemoryGraphStore::new());
    let primary_box: Box<dyn GraphStore> = Box::new(ArcStore(Arc::clone(&primary_inner)));

    let failing_inner = Arc::new(FailingStore::new());
    // Reuse the ArcFailingStore pattern from above — but since it's
    // defined inside the other test, define a minimal inline wrapper.
    struct ArcFS(Arc<FailingStore>);
    impl GraphStore for ArcFS {
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
        fn find_nodes_by_label(&self, g: &str, l: &str) -> Result<Vec<NodeId>, StorageError> {
            self.0.find_nodes_by_label(g, l)
        }
        fn find_nodes_by_label_and_property(
            &self,
            g: &str,
            l: &str,
            p: &str,
            r: RangeQuery<'_>,
        ) -> Result<Vec<NodeId>, StorageError> {
            self.0.find_nodes_by_label_and_property(g, l, p, r)
        }
        fn find_edges_by_type(&self, g: &str, t: &str) -> Result<Vec<EdgeId>, StorageError> {
            self.0.find_edges_by_type(g, t)
        }
        fn out_edges(&self, g: &str, n: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
            self.0.out_edges(g, n)
        }
        fn in_edges(&self, g: &str, n: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
            self.0.in_edges(g, n)
        }
        fn out_edges_by_type(
            &self,
            g: &str,
            n: NodeId,
            t: &[&str],
        ) -> Result<Vec<AdjEntry>, StorageError> {
            self.0.out_edges_by_type(g, n, t)
        }
        fn in_edges_by_type(
            &self,
            g: &str,
            n: NodeId,
            t: &[&str],
        ) -> Result<Vec<AdjEntry>, StorageError> {
            self.0.in_edges_by_type(g, n, t)
        }
        fn out_edge_ids(&self, g: &str, n: NodeId) -> Result<Vec<EdgeId>, StorageError> {
            self.0.out_edge_ids(g, n)
        }
        fn in_edge_ids(&self, g: &str, n: NodeId) -> Result<Vec<EdgeId>, StorageError> {
            self.0.in_edge_ids(g, n)
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
            l: Vec<String>,
            p: IndexMap<String, Value>,
        ) -> Result<NodeId, StorageError> {
            self.0.create_node(g, l, p)
        }
        fn create_edge(
            &self,
            g: &str,
            s: NodeId,
            d: NodeId,
            t: String,
            p: IndexMap<String, Value>,
        ) -> Result<EdgeId, StorageError> {
            self.0.create_edge(g, s, d, t, p)
        }
        fn set_node_properties(
            &self,
            g: &str,
            id: NodeId,
            p: IndexMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.0.set_node_properties(g, id, p)
        }
        fn replace_node_properties(
            &self,
            g: &str,
            id: NodeId,
            p: IndexMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.0.replace_node_properties(g, id, p)
        }
        fn set_edge_properties(
            &self,
            g: &str,
            id: EdgeId,
            p: IndexMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.0.set_edge_properties(g, id, p)
        }
        fn replace_edge_properties(
            &self,
            g: &str,
            id: EdgeId,
            p: IndexMap<String, Value>,
        ) -> Result<(), StorageError> {
            self.0.replace_edge_properties(g, id, p)
        }
        fn remove_node_property(&self, g: &str, id: NodeId, k: &str) -> Result<(), StorageError> {
            self.0.remove_node_property(g, id, k)
        }
        fn remove_edge_property(&self, g: &str, id: EdgeId, k: &str) -> Result<(), StorageError> {
            self.0.remove_edge_property(g, id, k)
        }
        fn add_node_labels(&self, g: &str, id: NodeId, l: &[String]) -> Result<(), StorageError> {
            self.0.add_node_labels(g, id, l)
        }
        fn remove_node_labels(
            &self,
            g: &str,
            id: NodeId,
            l: &[String],
        ) -> Result<(), StorageError> {
            self.0.remove_node_labels(g, id, l)
        }
        fn delete_node(&self, g: &str, id: NodeId, d: bool) -> Result<(), StorageError> {
            self.0.delete_node(g, id, d)
        }
        fn delete_edge(&self, g: &str, id: EdgeId) -> Result<(), StorageError> {
            self.0.delete_edge(g, id)
        }
        fn list_indexes(&self, g: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
            self.0.list_indexes(g)
        }
        fn create_index(&self, g: &str, l: &str, p: &str) -> Result<(), StorageError> {
            self.0.create_index(g, l, p)
        }
        fn drop_index(&self, g: &str, l: &str, p: &str) -> Result<(), StorageError> {
            self.0.drop_index(g, l, p)
        }
        fn create_graph(&self, n: &str) -> Result<(), StorageError> {
            self.0.create_graph(n)
        }
        fn delete_graph(&self, n: &str) -> Result<(), StorageError> {
            self.0.delete_graph(n)
        }
        fn graph_exists(&self, n: &str) -> Result<bool, StorageError> {
            self.0.graph_exists(n)
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

    let failing_box: Box<dyn GraphStore> = Box::new(ArcFS(Arc::clone(&failing_inner)));
    let fanout = FanoutStore::new(vec![primary_box, failing_box]);
    fanout.create_graph("g").unwrap();

    // Create two nodes (secondary is not failing yet, so this succeeds).
    let a = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let b = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();

    // Enable failures on secondary.
    failing_inner.enable_failures();

    // Attempt to create an edge — should fail.
    let result = fanout.create_edge("g", a, b, "KNOWS".to_string(), IndexMap::new());
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

#[test]
fn test_late_secondary_failure_rollback_create_edge_cleans_earlier_secondary() {
    let (primary_ref, primary_box) = shared_store();
    let (middle_ref, middle_box) = shared_store();
    let (late_ref, late_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, middle_box, late_box]);

    fanout.create_graph("g").unwrap();
    let a = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let b = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();

    late_ref.delete_graph("g").unwrap();

    let result = fanout.create_edge("g", a, b, "KNOWS".to_string(), IndexMap::new());
    assert!(
        result.is_err(),
        "create_edge should fail when a later secondary is out of sync"
    );

    assert!(
        primary_ref.all_edge_ids("g").unwrap().is_empty(),
        "primary edge should be rolled back after the late secondary failure"
    );
    assert!(
        middle_ref.all_edge_ids("g").unwrap().is_empty(),
        "earlier successful secondaries should also have the edge rolled back"
    );
    assert_eq!(
        primary_ref.all_node_ids("g").unwrap().len(),
        2,
        "nodes created before the failed edge write should remain on primary"
    );
    assert_eq!(
        middle_ref.all_node_ids("g").unwrap().len(),
        2,
        "nodes created before the failed edge write should remain on earlier secondaries"
    );
    assert!(
        !late_ref.graph_exists("g").unwrap(),
        "the failing late secondary should remain in its pre-failure state"
    );
}
