//! Query executor — evaluates a physical [`Operator`] tree against a [`GraphStore`].
//!
//! A [`Row`] is an owned map from column name to [`Value`].  During execution
//! node/edge variables hold `Value::Node(id)` / `Value::Edge(id)`.  When an
//! expression needs a property value (e.g. `n.name`) the executor resolves the
//! ID against the store and returns the concrete property value.
//!
//! Every operator checks `budget.check_or_err()` before doing any work.

mod aggregation;
mod decode;
mod eval;
pub(crate) mod helpers;
mod joins;
mod mutations;
mod produce;
mod scan;
mod scan_helpers;
mod shaping;

mod gremlin;

use std::collections::{HashMap, HashSet};

use crate::error::GqlError;
use crate::executor::budget::ExecutionBudget;
use crate::planner::Operator;
use crate::procedure::ProcedureRegistry;
use crate::storage::{EdgeData, EdgeId, GraphStore, NodeData, NodeId};
use crate::types::Value;
pub(crate) use decode::BfsArena;
pub use helpers::cmp_values;

// ── Public types ──────────────────────────────────────────────────────────────

/// A single result row: owned map from column name to value.
pub type Row = HashMap<String, Value>;

/// Bundles the per-call execution context that is threaded through every
/// operator invocation, replacing the previous 4-param `(graph, budget,
/// outer_rows, stats)` pattern.
pub(crate) struct ExecContext<'a> {
    pub graph: &'a str,
    pub budget: &'a mut ExecutionBudget,
    pub outer_rows: &'a [Row],
    pub stats: &'a mut MutationStats,
}

impl<'a> ExecContext<'a> {
    /// Reborrow with different `outer_rows`, used by Apply / LeftOuterApply.
    pub fn with_outer<'b>(&'b mut self, outer_rows: &'b [Row]) -> ExecContext<'b> {
        ExecContext {
            graph: self.graph,
            budget: &mut *self.budget,
            outer_rows,
            stats: &mut *self.stats,
        }
    }
}

// ── Result and stats types ────────────────────────────────────────────────────

/// The final result of executing a query.
#[derive(Debug)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
    pub stats: MutationStats,
    /// When a per-row error occurs during streaming evaluation (e.g. division
    /// by zero on one row of an UNWIND), the successfully-computed rows are
    /// returned in `rows` and the error that terminated evaluation is stored
    /// here. This allows Bolt-level streaming: the server can emit RECORD
    /// messages for the partial rows and then emit FAILURE for the trailing
    /// error.
    pub trailing_error: Option<GqlError>,
}

/// Counters for write operations performed during query execution.
#[derive(Debug, Default, Clone)]
pub struct MutationStats {
    pub nodes_created: u64,
    pub nodes_deleted: u64,
    pub edges_created: u64,
    pub edges_deleted: u64,
    pub properties_set: u64,
    pub labels_added: u64,
    pub labels_removed: u64,
    /// Internal: distinct label types seen during this execution.
    pub(crate) labels_seen: HashSet<String>,
    /// Internal: label types that existed in the graph before this query.
    pub(crate) pre_existing_labels: HashSet<String>,
    /// Internal: distinct (is_edge, entity_raw_id, prop_key) triples seen.
    pub(crate) props_seen: HashSet<(bool, u64, String)>,
    /// Internal: IDs of nodes created in this query (for net-effect tracking).
    pub(crate) nodes_created_ids: HashSet<u64>,
    /// Internal: IDs of edges created in this query (for net-effect tracking).
    pub(crate) edges_created_ids: HashSet<u64>,
}

impl MutationStats {
    /// Finalise derived counters from the tracking sets.
    /// Must be called once at the end of `execute()`.
    fn finalize(&mut self) {
        self.labels_added = self
            .labels_seen
            .difference(&self.pre_existing_labels)
            .count() as u64;
        self.properties_set = self.props_seen.len() as u64;
    }
}

// ── QueryExecutor ─────────────────────────────────────────────────────────────

/// Executes a physical plan against a [`GraphStore`].
pub struct QueryExecutor<'a> {
    pub(crate) store: &'a dyn GraphStore,
    pub(crate) registry: &'a ProcedureRegistry,
    /// Query parameters stored as `$name` → value.
    pub(crate) parameters: HashMap<String, Value>,
    /// Cache of deleted edge ID → rel_type string.
    /// Allows `type(r)` to work on a relationship deleted in the same query.
    pub(crate) deleted_edge_types: std::cell::RefCell<HashMap<u64, String>>,
    /// When a per-row error occurs in exec_project, the partial rows are
    /// returned as the "successful" result and the error is stashed here so
    /// that `execute()` can attach it to `QueryResult::trailing_error`.
    pub(crate) streaming_error: std::cell::RefCell<Option<GqlError>>,
    /// Property access cache for nodes. Avoids redundant store reads when
    /// the same node is accessed multiple times within a single read-only
    /// operator (e.g. `RETURN n.name, n.age, n.email`).
    /// Mutation operators invalidate this cache to prevent stale data.
    pub(crate) node_cache: std::cell::RefCell<HashMap<NodeId, NodeData>>,
    /// Property access cache for edges. Same rationale as `node_cache`.
    pub(crate) edge_cache: std::cell::RefCell<HashMap<EdgeId, EdgeData>>,
    /// When set, `exec_single_row` returns these rows instead of `[{}]`.
    /// Used by FOREACH to inject the loop variable binding into the body sub-plan.
    pub(crate) foreach_seed: std::cell::RefCell<Option<Vec<Row>>>,
    /// When true, `exec_project` preserves all input row columns in addition
    /// to the projected items.  This is required for Gremlin, where step
    /// labels set via `as()` must survive property projections.
    pub(crate) gremlin_mode: bool,
}

impl<'a> QueryExecutor<'a> {
    /// Create a new executor backed by `store`.
    pub fn new(store: &'a dyn GraphStore) -> Self {
        static EMPTY_REG: std::sync::OnceLock<ProcedureRegistry> = std::sync::OnceLock::new();
        Self {
            store,
            registry: EMPTY_REG.get_or_init(ProcedureRegistry::new),
            parameters: HashMap::new(),
            deleted_edge_types: std::cell::RefCell::new(HashMap::new()),
            streaming_error: std::cell::RefCell::new(None),
            node_cache: std::cell::RefCell::new(HashMap::new()),
            edge_cache: std::cell::RefCell::new(HashMap::new()),
            foreach_seed: std::cell::RefCell::new(None),
            gremlin_mode: false,
        }
    }

    /// Create an executor with a procedure registry and query parameters.
    pub fn with_context(
        store: &'a dyn GraphStore,
        registry: &'a ProcedureRegistry,
        parameters: HashMap<String, Value>,
    ) -> Self {
        // Convert parameter keys to $-prefixed format.
        let params = parameters
            .into_iter()
            .map(|(k, v)| {
                let key = if k.starts_with('$') {
                    k
                } else {
                    format!("${k}")
                };
                (key, v)
            })
            .collect();
        Self {
            store,
            registry,
            parameters: params,
            deleted_edge_types: std::cell::RefCell::new(HashMap::new()),
            streaming_error: std::cell::RefCell::new(None),
            node_cache: std::cell::RefCell::new(HashMap::new()),
            edge_cache: std::cell::RefCell::new(HashMap::new()),
            foreach_seed: std::cell::RefCell::new(None),
            gremlin_mode: false,
        }
    }

    /// Enable Gremlin mode: `Project` operators preserve all input columns
    /// so that step labels set via `as()` survive property projections.
    #[allow(dead_code)]
    pub fn with_gremlin_mode(mut self) -> Self {
        self.gremlin_mode = true;
        self
    }

    // ── Property access cache helpers ──────────────────────────────────────────

    /// Fetch a node's data, returning a cached copy if available.
    /// On a cache miss the node is fetched from the store and cached.
    pub(crate) fn cached_get_node(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Option<NodeData>, GqlError> {
        {
            let cache = self.node_cache.borrow();
            if let Some(data) = cache.get(&node_id) {
                return Ok(Some(data.clone()));
            }
        }
        let result = self.store.get_node(graph, node_id)?;
        if let Some(ref data) = result {
            self.node_cache.borrow_mut().insert(node_id, data.clone());
        }
        Ok(result)
    }

    /// Fetch an edge's data, returning a cached copy if available.
    /// On a cache miss the edge is fetched from the store and cached.
    pub(crate) fn cached_get_edge(
        &self,
        graph: &str,
        edge_id: EdgeId,
    ) -> Result<Option<EdgeData>, GqlError> {
        {
            let cache = self.edge_cache.borrow();
            if let Some(data) = cache.get(&edge_id) {
                return Ok(Some(data.clone()));
            }
        }
        let result = self.store.get_edge(graph, edge_id)?;
        if let Some(ref data) = result {
            self.edge_cache.borrow_mut().insert(edge_id, data.clone());
        }
        Ok(result)
    }

    /// Invalidate the property access caches.  Must be called after any
    /// mutation (CREATE, SET, REMOVE, DELETE, MERGE) to prevent stale reads.
    pub(crate) fn invalidate_property_cache(&self) {
        self.node_cache.borrow_mut().clear();
        self.edge_cache.borrow_mut().clear();
    }

    /// Execute `plan` against the named `graph`, enforcing `budget`.
    pub fn execute(
        &self,
        graph: &str,
        plan: &Operator,
        budget: &mut ExecutionBudget,
    ) -> Result<QueryResult, GqlError> {
        // RAII guard: initializes query-level "now" and ensures clear_query_now() is called
        // even if execution returns early via `?`.
        let _query_time_guard = crate::executor::filter::QueryTimeGuard::new();
        let pre_existing_labels: HashSet<String> =
            self.store.all_label_names(graph)?.into_iter().collect();
        let mut stats = MutationStats {
            pre_existing_labels,
            ..Default::default()
        };
        let rows = {
            let mut ctx = ExecContext {
                graph,
                budget,
                outer_rows: &[],
                stats: &mut stats,
            };
            self.exec_operator(&mut ctx, plan)?
        };
        stats.finalize();

        // Derive column list.  Handle the case where the root is Distinct{ProduceResults}.
        let effective_root = match plan {
            Operator::Distinct { child, .. } => child.as_ref(),
            other => other,
        };
        let columns = if let Operator::ProduceResults { columns, .. } = effective_root {
            columns.iter().map(|(name, _)| name.clone()).collect()
        } else {
            rows.first()
                .map(|r| {
                    let mut cols: Vec<String> = r.keys().cloned().collect();
                    cols.sort();
                    cols
                })
                .unwrap_or_default()
        };

        // Check if a per-row streaming error was stashed during execution.
        let trailing_error = self.streaming_error.borrow_mut().take();

        Ok(QueryResult {
            columns,
            rows,
            stats,
            trailing_error,
        })
    }

    // ── Internal dispatch ─────────────────────────────────────────────────────

    /// Recursively execute an operator, returning its output rows.
    ///
    /// When profiling is enabled on the budget, records the number of rows
    /// produced by each operator keyed by operator pointer address.
    #[allow(clippy::only_used_in_recursion)]
    pub(crate) fn exec_operator(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        cx.budget.check_or_err()?;

        let rows = match op {
            Operator::SingleRow => self.exec_single_row(cx, op),
            Operator::NodeScan { .. } => self.exec_node_scan(cx, op),
            Operator::Filter { .. } => self.exec_filter(cx, op),
            Operator::Expand { .. } => self.exec_expand(cx, op),
            Operator::Project { .. } => self.exec_project(cx, op),
            Operator::Distinct { .. } => self.exec_distinct(cx, op),
            Operator::Sort { .. } => self.exec_sort(cx, op),
            Operator::Limit { .. } => self.exec_limit(cx, op),
            Operator::Skip { .. } => self.exec_skip(cx, op),
            Operator::Aggregate { .. } => self.exec_aggregate(cx, op),
            Operator::Create { .. } => self.exec_create(cx, op),
            Operator::AssemblePath { .. } => self.exec_assemble_path(cx, op),
            Operator::Call { .. } => self.exec_call(cx, op),
            Operator::ProduceResults { .. } => self.exec_produce_results(cx, op),
            Operator::VarLenExpand { .. } => self.exec_var_len_expand(cx, op),
            Operator::Set { .. } => self.exec_set(cx, op),
            Operator::Remove { .. } => self.exec_remove(cx, op),
            Operator::Delete { .. } => self.exec_delete(cx, op),
            Operator::Merge { .. } => self.exec_merge(cx, op),
            Operator::MergeRelationship { .. } => self.exec_merge_rel(cx, op),
            Operator::Unwind { .. } => self.exec_unwind(cx, op),
            Operator::Union { .. } => self.exec_union(cx, op),
            Operator::LeftOuterApply { .. } => self.exec_left_outer_apply(cx, op),
            Operator::Apply { .. } => self.exec_apply(cx, op),
            Operator::IndexScan { .. } => self.exec_index_scan(cx, op),
            Operator::NodeByIdSeek { .. } => self.exec_node_by_id_seek(cx, op),
            Operator::ShortestPath { .. } => self.exec_shortest_path(cx, op),
            Operator::ForEach { .. } => self.exec_foreach(cx, op),
            Operator::Repeat { .. } => self.exec_repeat(cx, op),
            Operator::Choose { .. } => self.exec_choose(cx, op),
            Operator::ChooseMap { .. } => self.exec_choose_map(cx, op),
            Operator::Coalesce { .. } => self.exec_coalesce(cx, op),
            Operator::GremlinSideEffect { .. } => self.exec_gremlin_side_effect(cx, op),
            Operator::Cap { .. } => self.exec_cap(cx, op),
            Operator::GremlinAggregate { .. } => self.exec_gremlin_aggregate(cx, op),
            Operator::GremlinPath { .. } => self.exec_gremlin_path(cx, op),
            Operator::GremlinMatch { .. } => self.exec_gremlin_match(cx, op),
            Operator::GremlinSelect { .. } => self.exec_gremlin_select(cx, op),
            Operator::GremlinGroup { .. } => self.exec_gremlin_group(cx, op),
            Operator::GremlinMapSlice { .. } => self.exec_gremlin_map_slice(cx, op),
            Operator::GremlinSimplePath { .. } => self.exec_gremlin_simple_path(cx, op),
            Operator::GremlinCyclicPath { .. } => self.exec_gremlin_cyclic_path(cx, op),
            Operator::GremlinIndex { .. } => self.exec_gremlin_index(cx, op),
            Operator::GremlinProject { .. } => self.exec_gremlin_project(cx, op),
        }?;

        // Record per-operator row counts when profiling is enabled.
        if cx.budget.is_profiling() {
            let op_addr = op as *const Operator as usize;
            cx.budget.record_operator_rows(op_addr, rows.len());
        }

        Ok(rows)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Direction, Expr, Identifier, Label, RelType};
    use crate::executor::budget::ExecutionBudget;
    use crate::planner::{AggFunction, Operator, SetItem};
    use crate::storage::InMemoryGraphStore;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn produce_results_with_property() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ProduceResults {
            columns: vec![(
                "p.name".to_string(),
                Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("p"))),
                    "name".into(),
                ),
            )],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("p"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0]["p.name"], Value::Str("Alice".into()));
    }

    #[test]
    fn sort_operator() {
        let store = make_store();
        let names = ["Charlie", "Alice", "Bob"];
        for name in &names {
            let mut props = HashMap::new();
            props.insert("name".to_string(), Value::Str((*name).into()));
            store
                .create_node("g", vec!["Person".to_string()], props)
                .unwrap();
        }

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ProduceResults {
            columns: vec![(
                "p.name".to_string(),
                Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("p"))),
                    "name".into(),
                ),
            )],
            child: Box::new(Operator::Sort {
                items: vec![(
                    Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("p"))),
                        "name".into(),
                    ),
                    true,
                )],
                gremlin_order: false,
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("p"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 3);
        assert_eq!(result.rows[0]["p.name"], Value::Str("Alice".into()));
        assert_eq!(result.rows[1]["p.name"], Value::Str("Bob".into()));
        assert_eq!(result.rows[2]["p.name"], Value::Str("Charlie".into()));
    }

    #[test]
    fn limit_and_skip() {
        let store = make_store();
        for i in 0..5 {
            let mut props = HashMap::new();
            props.insert("i".to_string(), Value::Int(i));
            store.create_node("g", vec![], props).unwrap();
        }

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Limit {
            count: Expr::Integer(2),
            child: Box::new(Operator::Skip {
                count: Expr::Integer(1),
                child: Box::new(Operator::NodeScan {
                    label: None,
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn aggregate_count() {
        let store = make_store();
        for _ in 0..3 {
            store
                .create_node("g", vec!["Person".to_string()], HashMap::new())
                .unwrap();
        }

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ProduceResults {
            columns: vec![(
                "count".to_string(),
                Expr::Variable(Identifier::new("count")),
            )],
            child: Box::new(Operator::Aggregate {
                keys: vec![],
                aggregations: vec![(
                    AggFunction::Count,
                    Expr::Variable(Identifier::new("n")),
                    None,
                    false,
                    Identifier::new("count"),
                )],
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0]["count"], Value::Int(3));
    }

    #[test]
    fn varlen_expand_linear_chain() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let c = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, c, "R".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            min: 1,
            max: 2,
            src_alias: Identifier::new("a"),
            path_alias: Some(Identifier::new("p")),
            path_mode: true,
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
            rel_props: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 3);
        let paths: Vec<_> = result
            .rows
            .iter()
            .filter(|r| matches!(r.get("p"), Some(Value::Path { .. })))
            .collect();
        assert_eq!(paths.len(), 3);
    }

    #[test]
    fn varlen_expand_min_zero_includes_self() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));

        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![],
            min: 0,
            max: 1,
            src_alias: Identifier::new("src"),
            path_alias: None,
            path_mode: false,
            dst_alias: Identifier::new("dst"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("src"),
            }),
            rel_props: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert!(result.rows.len() >= 2);
    }

    #[test]
    fn varlen_expand_avoids_cycles() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, a, "R".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));

        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![],
            min: 1,
            max: 5,
            src_alias: Identifier::new("src"),
            path_alias: None,
            path_mode: false,
            dst_alias: Identifier::new("dst"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("src"),
            }),
            rel_props: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert!(!result.rows.is_empty());
        assert!(result.rows.len() < 100);
    }

    #[test]
    fn set_property() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let node_id = store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Set {
            items: vec![SetItem::Property {
                alias: Identifier::new("n"),
                key: "age".to_string(),
                value: Expr::Integer(30),
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.properties_set, 1);

        let node = store.get_node("g", node_id).unwrap().unwrap();
        assert_eq!(node.properties.get("age"), Some(&Value::Int(30)));
        assert_eq!(
            node.properties.get("name"),
            Some(&Value::Str("Alice".into()))
        );
    }

    #[test]
    fn set_labels() {
        let store = make_store();
        let node_id = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Set {
            items: vec![SetItem::Labels {
                alias: Identifier::new("n"),
                labels: vec![Label::new("Employee")],
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.labels_added, 1);

        let node = store.get_node("g", node_id).unwrap().unwrap();
        assert!(node.labels.contains(&"Employee".to_string()));
        assert!(node.labels.contains(&"Person".to_string()));
    }

    #[test]
    fn set_map_merge() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let node_id = store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Set {
            items: vec![SetItem::Map {
                alias: Identifier::new("n"),
                map: Expr::MapLiteral(vec![(smol_str::SmolStr::new("age"), Expr::Integer(25))]),
                merge: true,
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.properties_set, 1);

        let node = store.get_node("g", node_id).unwrap().unwrap();
        assert_eq!(
            node.properties.get("name"),
            Some(&Value::Str("Alice".into()))
        );
        assert_eq!(node.properties.get("age"), Some(&Value::Int(25)));
    }

    #[test]
    fn remove_property() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        props.insert("age".to_string(), Value::Int(30));
        let node_id = store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Remove {
            items: vec![crate::planner::RemoveItem::Property {
                alias: Identifier::new("n"),
                key: "age".to_string(),
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.properties_set, 0);

        let node = store.get_node("g", node_id).unwrap().unwrap();
        assert!(!node.properties.contains_key("age"));
        assert_eq!(
            node.properties.get("name"),
            Some(&Value::Str("Alice".into()))
        );
    }

    #[test]
    fn remove_labels() {
        let store = make_store();
        let node_id = store
            .create_node(
                "g",
                vec!["Person".to_string(), "Employee".to_string()],
                HashMap::new(),
            )
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Remove {
            items: vec![crate::planner::RemoveItem::Labels {
                alias: Identifier::new("n"),
                labels: vec![Label::new("Employee")],
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let _result = executor.execute("g", &plan, &mut budget).unwrap();

        let node = store.get_node("g", node_id).unwrap().unwrap();
        assert!(node.labels.contains(&"Person".to_string()));
        assert!(!node.labels.contains(&"Employee".to_string()));
    }

    #[test]
    fn delete_node_detach() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Delete {
            exprs: vec![Expr::Variable(Identifier::new("n"))],
            detach: true,
            child: Box::new(Operator::Filter {
                predicate: Expr::Bool(true),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_deleted, 2);
    }

    #[test]
    fn delete_null_is_noop() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Delete {
            exprs: vec![Expr::Null],
            detach: false,
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        store.create_node("g", vec![], HashMap::new()).unwrap();
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_deleted, 0);
    }

    #[test]
    fn delete_edge() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let eid = store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Delete {
            exprs: vec![Expr::Variable(Identifier::new("r"))],
            detach: false,
            child: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("R")],
                src_alias: Identifier::new("a"),
                rel_alias: Some(Identifier::new("r")),
                dst_alias: Identifier::new("b"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("a"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.edges_deleted, 1);
        assert!(store.get_edge("g", eid).unwrap().is_none());
    }

    #[test]
    fn merge_creates_when_not_found() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("Person")],
            properties: vec![],
            on_create: vec![SetItem::Property {
                alias: Identifier::new("n"),
                key: "name".to_string(),
                value: Expr::Str("Alice".into()),
            }],
            on_match: vec![],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 1);
        assert_eq!(result.stats.properties_set, 1);

        let ids = store.find_nodes_by_label("g", "Person").unwrap();
        assert_eq!(ids.len(), 1);
        let node = store.get_node("g", ids[0]).unwrap().unwrap();
        assert_eq!(
            node.properties.get("name"),
            Some(&Value::Str("Alice".into()))
        );
    }

    #[test]
    fn merge_matches_existing() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let existing = store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("Person")],
            properties: vec![],
            on_create: vec![],
            on_match: vec![SetItem::Property {
                alias: Identifier::new("n"),
                key: "seen".to_string(),
                value: Expr::Bool(true),
            }],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 0);
        assert_eq!(result.stats.properties_set, 1);

        let node = store.get_node("g", existing).unwrap().unwrap();
        assert_eq!(node.properties.get("seen"), Some(&Value::Bool(true)));
    }

    #[test]
    fn merge_without_child() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("City")],
            properties: vec![],
            on_create: vec![],
            on_match: vec![],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 1);
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("n"));
    }

    #[test]
    fn unwind_list() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Unwind {
            list_expr: Expr::List(vec![Expr::Integer(1), Expr::Integer(2), Expr::Integer(3)]),
            alias: Identifier::new("x"),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 3);
        let values: Vec<&Value> = result.rows.iter().map(|r| &r["x"]).collect();
        assert_eq!(values, vec![&Value::Int(1), &Value::Int(2), &Value::Int(3)]);
    }

    #[test]
    fn unwind_null_produces_zero_rows() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Unwind {
            list_expr: Expr::Null,
            alias: Identifier::new("x"),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn unwind_empty_list_produces_zero_rows() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Unwind {
            list_expr: Expr::List(vec![]),
            alias: Identifier::new("x"),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn union_all_concatenates() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["Movie".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Union {
            left: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
            right: Box::new(Operator::NodeScan {
                label: Some(Label::new("Movie")),
                alias: Identifier::new("n"),
            }),
            all: true,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn union_deduplicates() {
        let store = make_store();
        let _node = store
            .create_node("g", vec!["Both".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Union {
            left: Box::new(Operator::NodeScan {
                label: Some(Label::new("Both")),
                alias: Identifier::new("n"),
            }),
            right: Box::new(Operator::NodeScan {
                label: Some(Label::new("Both")),
                alias: Identifier::new("n"),
            }),
            all: false,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn left_outer_apply_with_match() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::LeftOuterApply {
            preserved: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("a"),
            }),
            optional_pattern: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("KNOWS")],
                src_alias: Identifier::new("a"),
                rel_alias: Some(Identifier::new("r")),
                dst_alias: Identifier::new("friend"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("a"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn left_outer_apply_no_match_produces_nulls() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::LeftOuterApply {
            preserved: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("a"),
            }),
            optional_pattern: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("KNOWS")],
                src_alias: Identifier::new("a"),
                rel_alias: None,
                dst_alias: Identifier::new("b"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("a"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].get("b"), Some(&Value::Null));
    }

    #[test]
    fn apply_combines_outer_inner() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["B".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Apply {
            outer: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("a"),
            }),
            inner: Box::new(Operator::NodeScan {
                label: Some(Label::new("B")),
                alias: Identifier::new("b"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("a"));
        assert!(result.rows[0].contains_key("b"));
    }

    #[test]
    fn apply_empty_inner_passes_outer() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Apply {
            outer: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("a"),
            }),
            inner: Box::new(Operator::NodeScan {
                label: Some(Label::new("NonExistent")),
                alias: Identifier::new("b"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn index_scan_finds_nodes() {
        let store = make_store();
        let mut p1 = HashMap::new();
        p1.insert("age".to_string(), Value::Int(30));
        let _a = store
            .create_node("g", vec!["Person".to_string()], p1)
            .unwrap();

        let mut p2 = HashMap::new();
        p2.insert("age".to_string(), Value::Int(25));
        let _b = store
            .create_node("g", vec!["Person".to_string()], p2)
            .unwrap();

        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::IndexScan {
            label: Label::new("Person"),
            property: "age".to_string(),
            value: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn index_scan_empty_result() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::IndexScan {
            label: Label::new("Person"),
            property: "nonexistent".to_string(),
            value: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn node_by_id_seek_found() {
        let store = make_store();
        let node_id = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(node_id.0 as i64),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0]["n"], Value::Node(node_id));
    }

    #[test]
    fn node_by_id_seek_not_found() {
        let store = make_store();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(99999),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn node_by_id_seek_negative_id_is_error() {
        let store = make_store();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(-1),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget);
        assert!(result.is_err());
    }

    #[test]
    fn shortest_path_finds_path() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let c = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, c, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, c, "R".to_string(), HashMap::new())
            .unwrap();

        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));
        start_row.insert("dst".to_string(), Value::Node(c));

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ShortestPath {
            src_alias: Identifier::new("src"),
            dst_alias: Identifier::new("dst"),
            rel_types: vec![],
            direction: Direction::Outgoing,
            min: 1,
            max: 10,
            path_alias: Identifier::new("p"),
            all_paths: false,
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let rows = executor
            .exec_operator(
                &mut ExecContext {
                    graph: "g",
                    budget: &mut budget,
                    outer_rows: &[start_row],
                    stats: &mut stats,
                },
                &plan,
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        match &rows[0]["p"] {
            Value::Path { nodes, edges } => {
                assert_eq!(edges.len(), 1);
                assert_eq!(nodes[0], a);
                assert_eq!(nodes[1], c);
            }
            other => panic!("expected Path, got {:?}", other),
        }
    }

    #[test]
    fn shortest_path_all_paths() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let c = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let d = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, c, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, d, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", d, c, "R".to_string(), HashMap::new())
            .unwrap();

        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));
        start_row.insert("dst".to_string(), Value::Node(c));

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ShortestPath {
            src_alias: Identifier::new("src"),
            dst_alias: Identifier::new("dst"),
            rel_types: vec![],
            direction: Direction::Outgoing,
            min: 1,
            max: 10,
            path_alias: Identifier::new("p"),
            all_paths: true,
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let rows = executor
            .exec_operator(
                &mut ExecContext {
                    graph: "g",
                    budget: &mut budget,
                    outer_rows: &[start_row],
                    stats: &mut stats,
                },
                &plan,
            )
            .unwrap();
        assert_eq!(rows.len(), 2);
        for row in &rows {
            match &row["p"] {
                Value::Path { edges, .. } => assert_eq!(edges.len(), 2),
                other => panic!("expected Path, got {:?}", other),
            }
        }
    }

    #[test]
    fn shortest_path_no_path_exists() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();

        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));
        start_row.insert("dst".to_string(), Value::Node(b));

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ShortestPath {
            src_alias: Identifier::new("src"),
            dst_alias: Identifier::new("dst"),
            rel_types: vec![],
            direction: Direction::Outgoing,
            min: 1,
            max: 5,
            path_alias: Identifier::new("p"),
            all_paths: false,
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let rows = executor
            .exec_operator(
                &mut ExecContext {
                    graph: "g",
                    budget: &mut budget,
                    outer_rows: &[start_row],
                    stats: &mut stats,
                },
                &plan,
            )
            .unwrap();
        assert_eq!(rows.len(), 0);
    }

    #[test]
    fn cmp_values_cross_type_ordering() {
        use std::cmp::Ordering;
        assert_eq!(
            cmp_values(&Value::Str("z".into()), &Value::Bool(true)),
            Ordering::Less
        );
        assert_eq!(
            cmp_values(&Value::Bool(false), &Value::Str("a".into())),
            Ordering::Greater
        );
    }

    #[test]
    fn cmp_values_nan_sorts_after_regular_float() {
        use std::cmp::Ordering;
        let nan = Value::Float(f64::NAN);
        let regular = Value::Float(1.0);
        assert_eq!(cmp_values(&nan, &regular), Ordering::Greater);
        assert_eq!(cmp_values(&regular, &nan), Ordering::Less);
    }

    #[test]
    fn cmp_values_list_lexicographic() {
        use std::cmp::Ordering;
        let short = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let long = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let diff = Value::List(vec![Value::Int(1), Value::Int(3)]);
        assert_eq!(cmp_values(&short, &long), Ordering::Less);
        assert_eq!(cmp_values(&short, &diff), Ordering::Less);
        assert_eq!(cmp_values(&short, &short.clone()), Ordering::Equal);
    }

    #[test]
    fn cmp_values_null_sorts_last() {
        use std::cmp::Ordering;
        assert_eq!(cmp_values(&Value::Null, &Value::Int(0)), Ordering::Greater);
        assert_eq!(
            cmp_values(&Value::Null, &Value::Str("z".into())),
            Ordering::Greater
        );
        assert_eq!(cmp_values(&Value::Bool(true), &Value::Null), Ordering::Less);
        assert_eq!(cmp_values(&Value::Null, &Value::Null), Ordering::Equal);
    }

    #[test]
    fn cmp_values_time_utc_normalized() {
        use std::cmp::Ordering;
        let a = Value::Time(jiff::civil::Time::new(10, 35, 0, 0).unwrap(), -28800);
        let d = Value::Time(jiff::civil::Time::new(12, 35, 15, 0).unwrap(), 18000);
        assert_eq!(
            cmp_values(&a, &d),
            Ordering::Greater,
            "A(18:35Z) should be > D(07:35Z)"
        );
        assert_eq!(
            cmp_values(&d, &a),
            Ordering::Less,
            "D(07:35Z) should be < A(18:35Z)"
        );

        let t1 = Value::Time(jiff::civil::Time::new(14, 0, 0, 0).unwrap(), 7200);
        let t2 = Value::Time(jiff::civil::Time::new(13, 0, 0, 0).unwrap(), 3600);
        assert_eq!(cmp_values(&t1, &t2), Ordering::Equal);
    }

    #[test]
    fn e2e_varlen_expand_through_planner() {
        use crate::parser::parse_query;
        use crate::planner::QueryPlanner;

        let store = make_store();
        let setup = "CREATE (a {name: 'A'}), (b {name: 'B'}), (c {name: 'C'}), (d {name: 'D'}) CREATE (a)-[:CONTAINS]->(b), (b)-[:CONTAINS]->(c), (c)-[:CONTAINS]->(d)";
        let ast = parse_query(setup).unwrap();
        let planner = QueryPlanner::new(&store, "g");
        let plan = planner.plan(&ast).unwrap();
        let mut budget = ExecutionBudget::unlimited();
        QueryExecutor::new(&store)
            .execute("g", &plan, &mut budget)
            .unwrap();

        let query = "MATCH (a {name: 'A'})-[*]->(x) RETURN x";
        let ast2 = parse_query(query).unwrap();
        let plan2 = planner.plan(&ast2).unwrap();
        let mut budget2 = ExecutionBudget::unlimited();
        let result = QueryExecutor::new(&store)
            .execute("g", &plan2, &mut budget2)
            .unwrap();
        assert_eq!(result.rows.len(), 3, "Expected 3 rows: B, C, D");
    }

    #[test]
    fn e2e_create_path_variable_returns_path() {
        use crate::parser::parse_query;
        use crate::planner::QueryPlanner;

        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");

        let query = "CREATE p = (a:X)-[r:Y]->(b:Z) RETURN p";
        let ast = parse_query(query).unwrap();
        let plan = planner.plan(&ast).unwrap();
        let mut budget = ExecutionBudget::unlimited();
        let result = QueryExecutor::new(&store)
            .execute("g", &plan, &mut budget)
            .unwrap();

        assert_eq!(
            result.rows.len(),
            1,
            "CREATE should produce exactly one row"
        );

        let row = &result.rows[0];
        let p = row.get("p").expect("row should contain path variable 'p'");
        match p {
            Value::Path { nodes, edges } => {
                assert_eq!(nodes.len(), 2, "path should have 2 nodes");
                assert_eq!(edges.len(), 1, "path should have 1 edge");
            }
            other => panic!("expected Value::Path, got {:?}", other),
        }
    }

    // ── PROFILE tests ─────────────────────────────────────────────────

    #[test]
    fn profiling_records_per_operator_row_counts() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ProduceResults {
            columns: vec![("n".to_string(), Expr::Variable(Identifier::new("n")))],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        budget.enable_profiling();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);

        let stats = budget.take_profile_stats().expect("profiling was enabled");
        // ProduceResults operator should have 2 rows
        let root_addr = &plan as *const Operator as usize;
        assert_eq!(
            stats[&root_addr].rows, 2,
            "ProduceResults should have produced 2 rows"
        );

        // NodeScan child should also have 2 rows
        if let Operator::ProduceResults { child, .. } = &plan {
            let child_addr = child.as_ref() as *const Operator as usize;
            assert_eq!(
                stats[&child_addr].rows, 2,
                "NodeScan should have produced 2 rows"
            );
        }
    }

    #[test]
    fn profiling_disabled_produces_no_stats() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::ProduceResults {
            columns: vec![("x".to_string(), Expr::Integer(1))],
            child: Box::new(Operator::SingleRow),
        };

        let mut budget = ExecutionBudget::unlimited();
        // profiling NOT enabled
        let _result = executor.execute("g", &plan, &mut budget).unwrap();
        assert!(
            budget.take_profile_stats().is_none(),
            "no stats when profiling is disabled"
        );
    }

    #[test]
    fn profiling_with_filter_records_both_operators() {
        use crate::ast::BinaryOp;

        let store = make_store();
        store
            .create_node(
                "g",
                vec!["Item".to_string()],
                HashMap::from([("val".to_string(), Value::Int(1))]),
            )
            .unwrap();
        store
            .create_node(
                "g",
                vec!["Item".to_string()],
                HashMap::from([("val".to_string(), Value::Int(2))]),
            )
            .unwrap();
        store
            .create_node(
                "g",
                vec!["Item".to_string()],
                HashMap::from([("val".to_string(), Value::Int(3))]),
            )
            .unwrap();

        let executor = QueryExecutor::new(&store);
        // MATCH (n:Item) WHERE n.val > 1 RETURN n
        let scan = Operator::NodeScan {
            label: Some(Label::new("Item")),
            alias: Identifier::new("n"),
        };
        let filter = Operator::Filter {
            predicate: Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("n"))),
                    "val".into(),
                )),
                right: Box::new(Expr::Integer(1)),
            },
            child: Box::new(scan),
        };
        let plan = Operator::ProduceResults {
            columns: vec![("n".to_string(), Expr::Variable(Identifier::new("n")))],
            child: Box::new(filter),
        };

        let mut budget = ExecutionBudget::unlimited();
        budget.enable_profiling();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2, "only items with val > 1");

        let stats = budget.take_profile_stats().unwrap();

        // NodeScan produces 3 rows (all items)
        if let Operator::ProduceResults { child, .. } = &plan {
            if let Operator::Filter { child: scan, .. } = child.as_ref() {
                let scan_addr = scan.as_ref() as *const Operator as usize;
                assert_eq!(stats[&scan_addr].rows, 3, "NodeScan should produce 3 rows");
            }
            // Filter produces 2 rows (only val > 1)
            let filter_addr = child.as_ref() as *const Operator as usize;
            assert_eq!(stats[&filter_addr].rows, 2, "Filter should produce 2 rows");
        }

        // ProduceResults also produces 2 rows
        let root_addr = &plan as *const Operator as usize;
        assert_eq!(stats[&root_addr].rows, 2);
    }
}
