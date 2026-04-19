// Tests for FanoutStore — split by concern.
//
// Helper structs (`ArcStore`, `FailingStore`) and helper fns (`mem_store`,
// `shared_store`) live here; they are visible to child test modules via
// normal Rust child-can-see-parent privacy.

// Re-exported with `pub(super)` so child test modules can `use super::*;`
// to pick up shared deps and helper types in one line.
pub(super) use std::sync::Arc;

pub(super) use indexmap::IndexMap;

pub(super) use cypher_core::error::StorageError;
pub(super) use cypher_core::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, InMemoryGraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery,
};
pub(super) use cypher_core::types::Value;

pub(super) use super::FanoutStore;

mod admin;
mod propagation;
mod rollback_edge;
mod rollback_node;

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
    fn add_node_labels(&self, g: &str, id: NodeId, labels: &[String]) -> Result<(), StorageError> {
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
        if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst) {
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
    fn find_edges_by_type(&self, g: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
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
        properties: IndexMap<String, Value>,
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
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.should_fail()?;
        self.inner.create_edge(g, src, dst, rel_type, properties)
    }
    fn set_node_properties(
        &self,
        g: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.should_fail()?;
        self.inner.set_node_properties(g, id, properties)
    }
    fn replace_node_properties(
        &self,
        g: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.should_fail()?;
        self.inner.replace_node_properties(g, id, properties)
    }
    fn set_edge_properties(
        &self,
        g: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.should_fail()?;
        self.inner.set_edge_properties(g, id, properties)
    }
    fn replace_edge_properties(
        &self,
        g: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.should_fail()?;
        self.inner.replace_edge_properties(g, id, properties)
    }
    fn remove_node_property(&self, g: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.should_fail()?;
        self.inner.remove_node_property(g, id, key)
    }
    fn remove_edge_property(&self, g: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.should_fail()?;
        self.inner.remove_edge_property(g, id, key)
    }
    fn add_node_labels(&self, g: &str, id: NodeId, labels: &[String]) -> Result<(), StorageError> {
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
