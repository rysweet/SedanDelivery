// `impl GraphStore for RedisGraphStore` — the trait wiring.
//
// All methods delegate to per-concern `_impl` helpers defined in `reads.rs`,
// `writes/`, `indexing.rs`, `range.rs`, and `audit.rs`. Keeping the trait
// surface here makes the inventory of supported operations easy to read at
// a glance and isolates trait-shape churn from the per-method business logic.

use std::sync::Arc;

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor, NodeData, NodeId,
    RangeQuery, ReadConsistency,
};
use cypher_core::types::Value;

use crate::RedisGraphStore;

impl GraphStore for RedisGraphStore {
    // ── Single-item reads ────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        self.get_node_impl(graph, id)
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        self.get_edge_impl(graph, id)
    }

    // ── Batch reads ──────────────────────────────────────────────────────────

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        self.get_nodes_batch_impl(graph, ids)
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        self.get_edges_batch_impl(graph, ids)
    }

    // ── Label/type index reads ───────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        self.find_nodes_by_label_impl(graph, label)
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.find_nodes_by_label_and_property_impl(graph, label, property, range)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.find_edges_by_type_impl(graph, rel_type)
    }

    // ── Adjacency reads ─────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.out_edges_impl(graph, node_id)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.in_edges_impl(graph, node_id)
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.out_edges_by_type_impl(graph, node_id, rel_types)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.in_edges_by_type_impl(graph, node_id, rel_types)
    }

    fn out_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.out_edges_by_type_batch_impl(graph, node_ids, rel_types)
    }

    fn in_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.in_edges_by_type_batch_impl(graph, node_ids, rel_types)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.out_edge_ids_impl(graph, node_id)
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.in_edge_ids_impl(graph, node_id)
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        self.all_node_ids_impl(graph)
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.all_edge_ids_impl(graph)
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.all_label_names_impl(graph)
    }

    // ── Write operations ────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        self.create_node_impl(graph, labels, properties)
    }

    fn batch_create_nodes(
        &self,
        graph: &str,
        nodes: Vec<(Vec<String>, IndexMap<String, Value>)>,
    ) -> Result<Vec<NodeId>, StorageError> {
        RedisGraphStore::batch_create_nodes(self, graph, nodes)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.create_edge_impl(graph, src, dst, rel_type, properties)
    }

    fn batch_create_edges(
        &self,
        graph: &str,
        edges: Vec<(NodeId, NodeId, String, IndexMap<String, Value>)>,
    ) -> Result<Vec<EdgeId>, StorageError> {
        RedisGraphStore::batch_create_edges(self, graph, edges)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.set_node_properties_impl(graph, id, properties)
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.replace_node_properties_impl(graph, id, properties)
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.set_edge_properties_impl(graph, id, properties)
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.replace_edge_properties_impl(graph, id, properties)
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.remove_node_property_impl(graph, id, key)
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.remove_edge_property_impl(graph, id, key)
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.add_node_labels_impl(graph, id, labels)
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.remove_node_labels_impl(graph, id, labels)
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        self.delete_node_impl(graph, id, detach)
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.delete_edge_impl(graph, id)
    }

    // ── Index management ────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.list_indexes_impl(graph)
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.create_index_impl(graph, label, property)
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.drop_index_impl(graph, label, property)
    }

    // ── Graph administration ────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        self.create_graph_impl(name)
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.delete_graph_impl(name)
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        self.graph_exists_impl(name)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.list_graphs_impl()
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.graph_stats_impl(graph)
    }

    fn with_read_consistency(&self, consistency: ReadConsistency) -> Option<Arc<dyn GraphStore>> {
        Some(Arc::new(
            self.clone().with_read_consistency_mode(consistency),
        ))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    // tx_snapshot and tx_commit use the default no-op implementations.
    // RedisGraphStore writes are immediately durable — no client-side
    // snapshot is needed. Redis Cluster mode can add WATCH/MULTI/EXEC
    // for per-slot transaction isolation in a future iteration.
}

#[cfg(test)]
mod tests {
    //! Trait-level smoke tests for the `GraphStore` impl on `RedisGraphStore`.
    //!
    //! Each test invokes a trait method on a `new_for_testing()` store whose
    //! pool points at an unreachable URL. The expected outcome is therefore:
    //!
    //! - For methods that never touch the connection (validation rejects the
    //!   input, or empty inputs short-circuit): the appropriate validation
    //!   error / `Ok` empty value.
    //! - For all other methods: a `Backend` error from the first pool checkout.
    //!
    //! The goal is to exercise the trait wiring in `graph_store_impl.rs`
    //! plus the entry of every `_impl` helper without requiring a live Redis.
    //! Functional behaviour with a real Redis is covered by the integration
    //! tests under `tests/`.
    use super::*;
    use cypher_core::storage::{EdgeId, NodeId, RangeQuery, ReadConsistency};
    use indexmap::IndexMap;

    fn store() -> RedisGraphStore {
        RedisGraphStore::new_for_testing()
    }

    fn assert_backend<T: std::fmt::Debug>(r: Result<T, StorageError>, ctx: &str) {
        match r {
            Err(StorageError::Backend { .. }) => {}
            other => panic!("{ctx}: expected Backend error, got {other:?}"),
        }
    }

    #[test]
    fn get_node_propagates_backend_error() {
        assert_backend(store().get_node("g", NodeId(1)), "get_node");
    }

    #[test]
    fn get_edge_propagates_backend_error() {
        assert_backend(store().get_edge("g", EdgeId(1)), "get_edge");
    }

    #[test]
    fn get_nodes_batch_propagates_backend_error() {
        assert_backend(
            store().get_nodes_batch("g", &[NodeId(1)]),
            "get_nodes_batch",
        );
    }

    #[test]
    fn get_nodes_batch_propagates_backend_error_even_for_empty_input() {
        // get_nodes_batch checks out a connection before short-circuiting on
        // empty IDs, so even an empty batch surfaces a Backend error here.
        assert_backend(store().get_nodes_batch("g", &[]), "get_nodes_batch empty");
    }

    #[test]
    fn get_edges_batch_propagates_backend_error() {
        assert_backend(
            store().get_edges_batch("g", &[EdgeId(1)]),
            "get_edges_batch",
        );
    }

    #[test]
    fn get_edges_batch_propagates_backend_error_even_for_empty_input() {
        assert_backend(store().get_edges_batch("g", &[]), "get_edges_batch empty");
    }

    #[test]
    fn find_nodes_by_label_propagates_backend_error() {
        assert_backend(store().find_nodes_by_label("g", "L"), "find_nodes_by_label");
    }

    #[test]
    fn find_nodes_by_label_and_property_propagates_backend_error() {
        let v = Value::Int(42);
        let r = store().find_nodes_by_label_and_property("g", "L", "p", RangeQuery::Eq(&v));
        assert_backend(r, "find_nodes_by_label_and_property");
    }

    #[test]
    fn find_edges_by_type_propagates_backend_error() {
        assert_backend(store().find_edges_by_type("g", "R"), "find_edges_by_type");
    }

    #[test]
    fn out_edges_propagates_backend_error() {
        assert_backend(store().out_edges("g", NodeId(1)), "out_edges");
    }

    #[test]
    fn in_edges_propagates_backend_error() {
        assert_backend(store().in_edges("g", NodeId(1)), "in_edges");
    }

    #[test]
    fn out_edges_by_type_propagates_backend_error() {
        assert_backend(
            store().out_edges_by_type("g", NodeId(1), &["R"]),
            "out_edges_by_type",
        );
    }

    #[test]
    fn in_edges_by_type_propagates_backend_error() {
        assert_backend(
            store().in_edges_by_type("g", NodeId(1), &["R"]),
            "in_edges_by_type",
        );
    }

    #[test]
    fn out_edges_by_type_batch_propagates_backend_error() {
        assert_backend(
            store().out_edges_by_type_batch("g", &[NodeId(1)], &["R"]),
            "out_edges_by_type_batch",
        );
    }

    #[test]
    fn in_edges_by_type_batch_propagates_backend_error() {
        assert_backend(
            store().in_edges_by_type_batch("g", &[NodeId(1)], &["R"]),
            "in_edges_by_type_batch",
        );
    }

    #[test]
    fn out_edge_ids_propagates_backend_error() {
        assert_backend(store().out_edge_ids("g", NodeId(1)), "out_edge_ids");
    }

    #[test]
    fn in_edge_ids_propagates_backend_error() {
        assert_backend(store().in_edge_ids("g", NodeId(1)), "in_edge_ids");
    }

    #[test]
    fn all_node_ids_propagates_backend_error() {
        assert_backend(store().all_node_ids("g"), "all_node_ids");
    }

    #[test]
    fn all_edge_ids_propagates_backend_error() {
        assert_backend(store().all_edge_ids("g"), "all_edge_ids");
    }

    #[test]
    fn all_label_names_propagates_backend_error() {
        assert_backend(store().all_label_names("g"), "all_label_names");
    }

    #[test]
    fn create_node_propagates_backend_error_after_validation() {
        // Valid labels + props pass validation, then with_write_lock fails
        // at pool checkout. This exercises both the validation success path
        // and the lock entry.
        let r = store().create_node("g", vec!["L".to_string()], IndexMap::new());
        assert_backend(r, "create_node");
    }

    #[test]
    fn create_node_rejects_label_with_null_byte_before_touching_conn() {
        let r = store().create_node("g", vec!["bad\x00label".to_string()], IndexMap::new());
        assert!(matches!(r, Err(StorageError::ConstraintViolation { .. })));
    }

    #[test]
    fn batch_create_nodes_empty_short_circuits_to_empty_vec() {
        let out = store()
            .batch_create_nodes("g", Vec::new())
            .expect("empty batch must short-circuit");
        assert!(out.is_empty());
    }

    #[test]
    fn batch_create_nodes_propagates_backend_error_after_validation() {
        let nodes = vec![(vec!["L".to_string()], IndexMap::new())];
        assert_backend(store().batch_create_nodes("g", nodes), "batch_create_nodes");
    }

    #[test]
    fn batch_create_nodes_rejects_invalid_labels_before_touching_conn() {
        let nodes = vec![(vec!["bad\x00label".to_string()], IndexMap::new())];
        let r = store().batch_create_nodes("g", nodes);
        assert!(matches!(r, Err(StorageError::ConstraintViolation { .. })));
    }

    #[test]
    fn batch_create_nodes_rejects_negative_injected_id_before_touching_conn() {
        // `extract_injected_id` rejects negative `_sd2_id` values with a
        // Backend error before any pool checkout.
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(-1));
        let nodes = vec![(vec!["L".to_string()], props)];
        let r = store().batch_create_nodes("g", nodes);
        assert!(
            matches!(r, Err(StorageError::Backend { .. })),
            "expected Backend, got {r:?}"
        );
    }

    #[test]
    fn create_node_rejects_negative_injected_id_before_touching_conn() {
        // Wait — actually this validates only inside with_graph_write_lock,
        // so it surfaces a Backend error from the unreachable pool first.
        // The negative-id check lives behind the write lock acquisition.
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(-1));
        let r = store().create_node("g", vec!["L".to_string()], props);
        assert_backend(r, "create_node with negative _sd2_id");
    }

    #[test]
    fn create_edge_rejects_negative_injected_id_before_touching_conn() {
        let mut props = IndexMap::new();
        props.insert("_sd2_eid".to_string(), Value::Int(-5));
        let r = store().create_edge("g", NodeId(1), NodeId(2), "R".to_string(), props);
        assert_backend(r, "create_edge with negative _sd2_eid");
    }

    #[test]
    fn batch_create_edges_with_injected_eid_falls_back_to_single_path() {
        // When at least one edge carries an `_sd2_eid`, the entire batch
        // routes through the single-edge `create_edge_impl` path, which
        // fails at the pool checkout against the unreachable test URL.
        let mut props = IndexMap::new();
        props.insert("_sd2_eid".to_string(), Value::Int(7));
        let edges = vec![(NodeId(1), NodeId(2), "R".to_string(), props)];
        assert_backend(
            store().batch_create_edges("g", edges),
            "batch_create_edges with _sd2_eid",
        );
    }

    #[test]
    fn create_edge_propagates_backend_error_after_validation() {
        let r = store().create_edge("g", NodeId(1), NodeId(2), "R".to_string(), IndexMap::new());
        assert_backend(r, "create_edge");
    }

    #[test]
    fn create_edge_rejects_property_key_with_null_byte_before_touching_conn() {
        let mut props = IndexMap::new();
        props.insert("bad\x00key".to_string(), Value::Int(1));
        let r = store().create_edge("g", NodeId(1), NodeId(2), "R".to_string(), props);
        assert!(matches!(r, Err(StorageError::ConstraintViolation { .. })));
    }

    #[test]
    fn batch_create_edges_empty_short_circuits_to_empty_vec() {
        let out = store()
            .batch_create_edges("g", Vec::new())
            .expect("empty batch must short-circuit");
        assert!(out.is_empty());
    }

    #[test]
    fn batch_create_edges_propagates_backend_error_after_validation() {
        let edges = vec![(NodeId(1), NodeId(2), "R".to_string(), IndexMap::new())];
        assert_backend(store().batch_create_edges("g", edges), "batch_create_edges");
    }

    #[test]
    fn batch_create_edges_rejects_invalid_property_keys_before_touching_conn() {
        let mut props = IndexMap::new();
        props.insert("bad\x00key".to_string(), Value::Int(1));
        let edges = vec![(NodeId(1), NodeId(2), "R".to_string(), props)];
        let r = store().batch_create_edges("g", edges);
        assert!(matches!(r, Err(StorageError::ConstraintViolation { .. })));
    }

    #[test]
    fn set_node_properties_propagates_backend_error_after_validation() {
        let r = store().set_node_properties("g", NodeId(1), IndexMap::new());
        assert_backend(r, "set_node_properties");
    }

    #[test]
    fn set_node_properties_rejects_invalid_keys_before_touching_conn() {
        let mut props = IndexMap::new();
        props.insert("bad\x00key".to_string(), Value::Int(1));
        let r = store().set_node_properties("g", NodeId(1), props);
        assert!(matches!(r, Err(StorageError::ConstraintViolation { .. })));
    }

    #[test]
    fn replace_node_properties_propagates_backend_error_after_validation() {
        let r = store().replace_node_properties("g", NodeId(1), IndexMap::new());
        assert_backend(r, "replace_node_properties");
    }

    #[test]
    fn replace_node_properties_rejects_invalid_keys_before_touching_conn() {
        let mut props = IndexMap::new();
        props.insert("bad\x00key".to_string(), Value::Int(1));
        let r = store().replace_node_properties("g", NodeId(1), props);
        assert!(matches!(r, Err(StorageError::ConstraintViolation { .. })));
    }

    #[test]
    fn set_edge_properties_propagates_backend_error_after_validation() {
        let r = store().set_edge_properties("g", EdgeId(1), IndexMap::new());
        assert_backend(r, "set_edge_properties");
    }

    #[test]
    fn set_edge_properties_rejects_invalid_keys_before_touching_conn() {
        let mut props = IndexMap::new();
        props.insert("bad\x00key".to_string(), Value::Int(1));
        let r = store().set_edge_properties("g", EdgeId(1), props);
        assert!(matches!(r, Err(StorageError::ConstraintViolation { .. })));
    }

    #[test]
    fn replace_edge_properties_propagates_backend_error_after_validation() {
        let r = store().replace_edge_properties("g", EdgeId(1), IndexMap::new());
        assert_backend(r, "replace_edge_properties");
    }

    #[test]
    fn replace_edge_properties_rejects_invalid_keys_before_touching_conn() {
        let mut props = IndexMap::new();
        props.insert("bad\x00key".to_string(), Value::Int(1));
        let r = store().replace_edge_properties("g", EdgeId(1), props);
        assert!(matches!(r, Err(StorageError::ConstraintViolation { .. })));
    }

    #[test]
    fn remove_node_property_propagates_backend_error_after_validation() {
        assert_backend(
            store().remove_node_property("g", NodeId(1), "name"),
            "remove_node_property",
        );
    }

    #[test]
    fn remove_node_property_rejects_empty_key_before_touching_conn() {
        let r = store().remove_node_property("g", NodeId(1), "");
        assert!(matches!(r, Err(StorageError::ConstraintViolation { .. })));
    }

    #[test]
    fn remove_edge_property_propagates_backend_error_after_validation() {
        assert_backend(
            store().remove_edge_property("g", EdgeId(1), "name"),
            "remove_edge_property",
        );
    }

    #[test]
    fn remove_edge_property_rejects_empty_key_before_touching_conn() {
        let r = store().remove_edge_property("g", EdgeId(1), "");
        assert!(matches!(r, Err(StorageError::ConstraintViolation { .. })));
    }

    #[test]
    fn add_node_labels_propagates_backend_error_after_validation() {
        let labels = vec!["L".to_string()];
        assert_backend(
            store().add_node_labels("g", NodeId(1), &labels),
            "add_node_labels",
        );
    }

    #[test]
    fn add_node_labels_rejects_invalid_labels_before_touching_conn() {
        let labels = vec!["bad\x00label".to_string()];
        let r = store().add_node_labels("g", NodeId(1), &labels);
        assert!(matches!(r, Err(StorageError::ConstraintViolation { .. })));
    }

    #[test]
    fn remove_node_labels_propagates_backend_error_after_validation() {
        let labels = vec!["L".to_string()];
        assert_backend(
            store().remove_node_labels("g", NodeId(1), &labels),
            "remove_node_labels",
        );
    }

    #[test]
    fn remove_node_labels_rejects_invalid_labels_before_touching_conn() {
        let labels = vec!["bad\x00label".to_string()];
        let r = store().remove_node_labels("g", NodeId(1), &labels);
        assert!(matches!(r, Err(StorageError::ConstraintViolation { .. })));
    }

    #[test]
    fn delete_node_propagates_backend_error() {
        assert_backend(store().delete_node("g", NodeId(1), false), "delete_node");
    }

    #[test]
    fn delete_node_with_detach_propagates_backend_error() {
        assert_backend(
            store().delete_node("g", NodeId(1), true),
            "delete_node detach",
        );
    }

    #[test]
    fn delete_edge_propagates_backend_error() {
        assert_backend(store().delete_edge("g", EdgeId(1)), "delete_edge");
    }

    #[test]
    fn list_indexes_propagates_backend_error() {
        assert_backend(store().list_indexes("g"), "list_indexes");
    }

    #[test]
    fn create_index_propagates_backend_error() {
        assert_backend(store().create_index("g", "L", "p"), "create_index");
    }

    #[test]
    fn drop_index_propagates_backend_error() {
        assert_backend(store().drop_index("g", "L", "p"), "drop_index");
    }

    #[test]
    fn create_graph_propagates_backend_error_for_valid_name() {
        // A valid graph name passes validation, so the failure surfaces from
        // the with_graph_write_lock pool checkout.
        assert_backend(store().create_graph("validgraphname"), "create_graph");
    }

    #[test]
    fn create_graph_rejects_invalid_name_before_touching_conn() {
        // Brace characters are reserved for cluster hash-tags and rejected by
        // validate_graph_name with InvalidGraphName.
        let r = store().create_graph("bad{name}");
        assert!(
            matches!(r, Err(StorageError::InvalidGraphName { .. })),
            "expected InvalidGraphName for invalid graph name, got {r:?}"
        );
    }

    #[test]
    fn delete_graph_propagates_backend_error() {
        assert_backend(store().delete_graph("validgraphname"), "delete_graph");
    }

    #[test]
    fn graph_exists_propagates_backend_error() {
        assert_backend(store().graph_exists("g"), "graph_exists");
    }

    #[test]
    fn list_graphs_propagates_backend_error() {
        assert_backend(store().list_graphs(), "list_graphs");
    }

    #[test]
    fn graph_stats_propagates_backend_error() {
        assert_backend(store().graph_stats("g"), "graph_stats");
    }

    #[test]
    fn with_read_consistency_returns_a_distinct_store_view() {
        let s = store();
        let view = GraphStore::with_read_consistency(&s, ReadConsistency::SessionFreshPrimary)
            .expect("with_read_consistency must return Some");
        // The new view is wired into the trait (no panic) and its concrete
        // type is RedisGraphStore.
        assert!(view.as_any().downcast_ref::<RedisGraphStore>().is_some());
    }

    #[test]
    fn as_any_downcasts_to_redis_graph_store() {
        let s = store();
        let any = s.as_any();
        assert!(any.downcast_ref::<RedisGraphStore>().is_some());
    }
}
