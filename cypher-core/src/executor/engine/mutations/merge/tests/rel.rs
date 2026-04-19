//! Tests for `MERGE (a)-[r:R]->(b)` relationship merging, including the
//! batched fast-path adjacency lookup.

use indexmap::IndexMap;
use std::any::Any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::super::MergeRelBatchSpec;
use super::make_store;
use crate::ast::{Direction, Expr, Identifier, Label, RelType};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::{ExecContext, QueryExecutor, Row};
use crate::planner::Operator;
use crate::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, InMemoryGraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery, ReadConsistency,
};
use crate::types::Value;

#[test]
fn merge_rel_creates_when_not_found() {
    let store = make_store();
    let _a = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let _b = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::MergeRelationship {
        src_alias: Identifier::new("a"),
        dst_alias: Identifier::new("b"),
        rel_alias: Some(Identifier::new("r")),
        rel_type: RelType::new("KNOWS"),
        direction: Direction::Outgoing,
        properties: vec![],
        on_create: vec![],
        on_match: vec![],
        child: Box::new(Operator::Apply {
            outer: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
            inner: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("b"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // With 2 nodes, Apply produces rows for each (a,b) pair where a!=b
    // (since inner NodeScan with outer_rows only produces matching nodes)
    // Actually it produces all combinations. Let's just check edges were created.
    assert!(result.stats.edges_created > 0);
}

#[test]
fn merge_rel_matches_existing() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["B".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let plan = Operator::MergeRelationship {
        src_alias: Identifier::new("a"),
        dst_alias: Identifier::new("b"),
        rel_alias: Some(Identifier::new("r")),
        rel_type: RelType::new("R"),
        direction: Direction::Outgoing,
        properties: vec![],
        on_create: vec![],
        on_match: vec![],
        child: Box::new(Operator::Apply {
            outer: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("a"),
            }),
            inner: Box::new(Operator::NodeScan {
                label: Some(Label::new("B")),
                alias: Identifier::new("b"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.stats.edges_created, 0,
        "existing edge should be matched, not created"
    );
}

#[test]
fn merge_rel_batch_path_deduplicates_duplicate_input_rows() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["B".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let plan = Operator::MergeRelationship {
        src_alias: Identifier::new("a"),
        dst_alias: Identifier::new("b"),
        rel_alias: Some(Identifier::new("r")),
        rel_type: RelType::new("R"),
        direction: Direction::Outgoing,
        properties: vec![],
        on_create: vec![],
        on_match: vec![],
        child: Box::new(Operator::Unwind {
            list_expr: Expr::List(vec![Expr::Integer(1), Expr::Integer(2)]),
            alias: Identifier::new("dup"),
            child: Box::new(Operator::Apply {
                outer: Box::new(Operator::NodeByIdSeek {
                    id_expr: Expr::Integer(a.0 as i64),
                    alias: Identifier::new("a"),
                }),
                inner: Box::new(Operator::NodeByIdSeek {
                    id_expr: Expr::Integer(b.0 as i64),
                    alias: Identifier::new("b"),
                }),
            }),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.edges_created, 1);
    assert_eq!(result.rows.len(), 2);

    let edge_ids = store.find_edges_by_type("g", "R").unwrap();
    assert_eq!(
        edge_ids.len(),
        1,
        "duplicate MERGE rows must reuse one edge"
    );

    let rel_values: Vec<_> = result
        .rows
        .iter()
        .map(|row| row.get("r").cloned().expect("merge should bind rel alias"))
        .collect();
    assert_eq!(rel_values[0], rel_values[1]);
}

#[test]
fn merge_rel_fast_and_slow_paths_share_duplicate_key_semantics() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["B".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_edge(
            "g",
            a,
            b,
            "R".to_string(),
            IndexMap::from([("dup".to_string(), Value::Int(2))]),
        )
        .unwrap();

    let duplicate_key_props = vec![
        ("dup".to_string(), Expr::Integer(1)),
        ("dup".to_string(), Expr::Integer(2)),
    ];

    let fast_executor = QueryExecutor::new(&store);
    let fast_plan = Operator::MergeRelationship {
        src_alias: Identifier::new("a"),
        dst_alias: Identifier::new("b"),
        rel_alias: Some(Identifier::new("r")),
        rel_type: RelType::new("R"),
        direction: Direction::Outgoing,
        properties: duplicate_key_props.clone(),
        on_create: vec![],
        on_match: vec![],
        child: Box::new(Operator::Apply {
            outer: Box::new(Operator::NodeByIdSeek {
                id_expr: Expr::Integer(a.0 as i64),
                alias: Identifier::new("a"),
            }),
            inner: Box::new(Operator::NodeByIdSeek {
                id_expr: Expr::Integer(b.0 as i64),
                alias: Identifier::new("b"),
            }),
        }),
    };
    let mut fast_budget = ExecutionBudget::unlimited();
    let fast_result = fast_executor
        .execute("g", &fast_plan, &mut fast_budget)
        .unwrap();

    let slow_executor = QueryExecutor::new(&store);
    let slow_plan = Operator::MergeRelationship {
        src_alias: Identifier::new("a"),
        dst_alias: Identifier::new("b"),
        rel_alias: Some(Identifier::new("r")),
        rel_type: RelType::new("R"),
        direction: Direction::Outgoing,
        properties: duplicate_key_props,
        on_create: vec![crate::planner::SetItem::Property {
            alias: Identifier::new("r"),
            key: "seen".to_string(),
            value: Expr::Bool(true),
        }],
        on_match: vec![],
        child: Box::new(Operator::Apply {
            outer: Box::new(Operator::NodeByIdSeek {
                id_expr: Expr::Integer(a.0 as i64),
                alias: Identifier::new("a"),
            }),
            inner: Box::new(Operator::NodeByIdSeek {
                id_expr: Expr::Integer(b.0 as i64),
                alias: Identifier::new("b"),
            }),
        }),
    };
    let mut slow_budget = ExecutionBudget::unlimited();
    let slow_result = slow_executor
        .execute("g", &slow_plan, &mut slow_budget)
        .unwrap();

    assert_eq!(fast_result.stats.edges_created, 0);
    assert_eq!(slow_result.stats.edges_created, 0);
    assert_eq!(
        fast_result.rows[0].get("r"),
        slow_result.rows[0].get("r"),
        "fast and slow MERGE paths must match the same existing edge"
    );
}

// ── TrackingStore for verifying batch call usage ─────────────────────────

struct TrackingStore {
    inner: Arc<InMemoryGraphStore>,
    out_edges_by_type_calls: AtomicUsize,
    out_edges_by_type_batch_calls: AtomicUsize,
    get_edges_batch_calls: AtomicUsize,
}

impl TrackingStore {
    fn new(inner: Arc<InMemoryGraphStore>) -> Self {
        Self {
            inner,
            out_edges_by_type_calls: AtomicUsize::new(0),
            out_edges_by_type_batch_calls: AtomicUsize::new(0),
            get_edges_batch_calls: AtomicUsize::new(0),
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

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, crate::error::StorageError> {
        self.get_edges_batch_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.get_edges_batch(graph, ids)
    }

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

#[test]
fn merge_rel_batch_uses_batch_adjacency_lookup() {
    // Verify that exec_merge_rel_batch_creates uses out_edges_by_type_batch
    // instead of per-row out_edges_by_type calls.
    let base = Arc::new(InMemoryGraphStore::new());
    base.create_graph("g").unwrap();

    // Create 20 nodes.
    let mut node_ids = Vec::new();
    for _ in 0..20 {
        node_ids.push(
            base.create_node("g", vec!["N".to_string()], IndexMap::new())
                .unwrap(),
        );
    }

    // Pre-create 5 edges so some MERGEs match.
    for i in 0..5 {
        base.create_edge(
            "g",
            node_ids[i],
            node_ids[i + 1],
            "R".to_string(),
            IndexMap::new(),
        )
        .unwrap();
    }

    let store = TrackingStore::new(Arc::clone(&base));
    let executor = QueryExecutor::new(&store);

    use crate::executor::engine::MutationStats;

    let mut budget = ExecutionBudget::unlimited();
    let mut stats = MutationStats::default();
    let outer_rows: Vec<Row> = vec![];
    let mut cx = ExecContext {
        graph: "g",
        budget: &mut budget,
        outer_rows: &outer_rows,
        stats: &mut stats,
    };

    let mut input_rows = Vec::new();
    for i in 0..15 {
        let mut row = Row::new();
        row.insert("a".to_string(), Value::Node(node_ids[i]));
        row.insert("b".to_string(), Value::Node(node_ids[i + 1]));
        input_rows.push(row);
    }

    let spec = MergeRelBatchSpec {
        src_alias: &Identifier::new("a"),
        dst_alias: &Identifier::new("b"),
        rel_alias: Some(&Identifier::new("r")),
        rel_type: &RelType::new("R"),
        direction: Direction::Outgoing,
        properties: &[],
    };

    let result = executor
        .exec_merge_rel_batch_creates(&mut cx, input_rows, spec)
        .unwrap();

    // All 15 rows should produce output.
    assert_eq!(result.len(), 15);
    // 10 new edges created (pairs 5..15).
    assert_eq!(cx.stats.edges_created, 10);

    // The batch path should be used: at least 1 batch call, zero single calls.
    let single_calls = store.out_edges_by_type_calls.load(Ordering::Relaxed);
    let batch_calls = store.out_edges_by_type_batch_calls.load(Ordering::Relaxed);
    assert_eq!(
        single_calls, 0,
        "Expected 0 single out_edges_by_type calls, got {single_calls}"
    );
    assert!(
        batch_calls >= 1,
        "Expected at least 1 batch call, got {batch_calls}"
    );
}
