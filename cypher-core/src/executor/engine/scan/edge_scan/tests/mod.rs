//! Shared test helpers + per-operator test sub-modules.

pub(super) use crate::ast::{Direction, Expr, Identifier, Label, RelType};
pub(super) use crate::error::{ExecutionError, GqlError};
pub(super) use crate::executor::budget::ExecutionBudget;
pub(super) use crate::executor::engine::{ExecContext, MutationStats, QueryExecutor, Row};
pub(super) use crate::planner::Operator;
pub(super) use crate::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, InMemoryGraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery, ReadConsistency,
};
pub(super) use crate::types::Value;
pub(super) use indexmap::IndexMap;
pub(super) use std::any::Any;
pub(super) use std::sync::atomic::{AtomicUsize, Ordering};
pub(super) use std::sync::Arc;

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

fn executor_result(
    store: &InMemoryGraphStore,
    plan: &Operator,
    budget: &mut ExecutionBudget,
) -> Vec<Row> {
    let executor = QueryExecutor::new(store);
    executor.execute("g", plan, budget).unwrap().rows
}

fn executor_result_gremlin(
    store: &InMemoryGraphStore,
    plan: &Operator,
    budget: &mut ExecutionBudget,
) -> Vec<Row> {
    let executor = QueryExecutor::new(store).with_gremlin_mode();
    executor.execute("g", plan, budget).unwrap().rows
}

struct TrackingStore {
    inner: Arc<InMemoryGraphStore>,
    out_edges_by_type_calls: AtomicUsize,
    out_edges_by_type_batch_calls: AtomicUsize,
}

impl TrackingStore {
    fn new(inner: Arc<InMemoryGraphStore>) -> Self {
        Self {
            inner,
            out_edges_by_type_calls: AtomicUsize::new(0),
            out_edges_by_type_batch_calls: AtomicUsize::new(0),
        }
    }
}

macro_rules! delegate {
    ($name:ident(&self) -> $ret:ty) => {
        fn $name(&self) -> $ret {
            self.inner.$name()
        }
    };
    ($name:ident(&self, $($arg:ident : $ty:ty),+) -> $ret:ty) => {
        fn $name(&self, $($arg: $ty),+) -> $ret {
            self.inner.$name($($arg),+)
        }
    };
}

impl GraphStore for TrackingStore {
    delegate!(get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, crate::error::StorageError>);
    delegate!(get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, crate::error::StorageError>);
    delegate!(get_nodes_batch(&self, graph: &str, ids: &[NodeId]) -> Result<Vec<Option<NodeData>>, crate::error::StorageError>);
    delegate!(get_edges_batch(&self, graph: &str, ids: &[EdgeId]) -> Result<Vec<Option<EdgeData>>, crate::error::StorageError>);
    delegate!(find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, crate::error::StorageError>);

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, crate::error::StorageError> {
        self.inner
            .find_nodes_by_label_and_property(graph, label, property, range)
    }

    delegate!(find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, crate::error::StorageError>);
    delegate!(out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, crate::error::StorageError>);
    delegate!(in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, crate::error::StorageError>);

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, crate::error::StorageError> {
        self.out_edges_by_type_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.out_edges_by_type(graph, node_id, rel_types)
    }

    delegate!(in_edges_by_type(&self, graph: &str, node_id: NodeId, rel_types: &[&str]) -> Result<Vec<AdjEntry>, crate::error::StorageError>);

    fn out_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, crate::error::StorageError> {
        self.out_edges_by_type_batch_calls
            .fetch_add(1, Ordering::Relaxed);
        self.inner
            .out_edges_by_type_batch(graph, node_ids, rel_types)
    }

    delegate!(in_edges_by_type_batch(&self, graph: &str, node_ids: &[NodeId], rel_types: &[&str]) -> Result<Vec<Vec<AdjEntry>>, crate::error::StorageError>);
    delegate!(out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, crate::error::StorageError>);
    delegate!(in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, crate::error::StorageError>);
    delegate!(all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, crate::error::StorageError>);
    delegate!(all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, crate::error::StorageError>);
    delegate!(create_node(&self, graph: &str, labels: Vec<String>, properties: IndexMap<String, Value>) -> Result<NodeId, crate::error::StorageError>);
    delegate!(create_edge(&self, graph: &str, src: NodeId, dst: NodeId, rel_type: String, properties: IndexMap<String, Value>) -> Result<EdgeId, crate::error::StorageError>);
    delegate!(set_node_properties(&self, graph: &str, id: NodeId, properties: IndexMap<String, Value>) -> Result<(), crate::error::StorageError>);
    delegate!(replace_node_properties(&self, graph: &str, id: NodeId, properties: IndexMap<String, Value>) -> Result<(), crate::error::StorageError>);
    delegate!(set_edge_properties(&self, graph: &str, id: EdgeId, properties: IndexMap<String, Value>) -> Result<(), crate::error::StorageError>);
    delegate!(replace_edge_properties(&self, graph: &str, id: EdgeId, properties: IndexMap<String, Value>) -> Result<(), crate::error::StorageError>);
    delegate!(remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), crate::error::StorageError>);
    delegate!(remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), crate::error::StorageError>);
    delegate!(add_node_labels(&self, graph: &str, id: NodeId, labels: &[String]) -> Result<(), crate::error::StorageError>);
    delegate!(remove_node_labels(&self, graph: &str, id: NodeId, labels: &[String]) -> Result<(), crate::error::StorageError>);
    delegate!(delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), crate::error::StorageError>);
    delegate!(delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), crate::error::StorageError>);
    delegate!(list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, crate::error::StorageError>);
    delegate!(create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), crate::error::StorageError>);
    delegate!(drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), crate::error::StorageError>);
    delegate!(create_graph(&self, name: &str) -> Result<(), crate::error::StorageError>);
    delegate!(delete_graph(&self, name: &str) -> Result<(), crate::error::StorageError>);
    delegate!(graph_exists(&self, name: &str) -> Result<bool, crate::error::StorageError>);
    delegate!(list_graphs(&self) -> Result<Vec<String>, crate::error::StorageError>);
    delegate!(graph_stats(&self, graph: &str) -> Result<GraphStats, crate::error::StorageError>);
    delegate!(all_label_names(&self, graph: &str) -> Result<Vec<String>, crate::error::StorageError>);

    fn with_read_consistency(&self, _consistency: ReadConsistency) -> Option<Arc<dyn GraphStore>> {
        None
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

mod expand;
mod shortest_path;
mod var_len;
