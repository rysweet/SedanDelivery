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
        }
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
        let rows = self.exec_operator(graph, plan, budget, &[], &mut stats)?;
        stats.finalize();

        // Derive column list.  Handle the case where the root is Distinct{ProduceResults}.
        let effective_root = match plan {
            Operator::Distinct { child } => child.as_ref(),
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
    #[allow(clippy::only_used_in_recursion)]
    pub(crate) fn exec_operator(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        budget.check_or_err()?;

        match op {
            Operator::SingleRow => self.exec_single_row(graph, op, budget, outer_rows, stats),
            Operator::NodeScan { .. } => self.exec_node_scan(graph, op, budget, outer_rows, stats),
            Operator::Filter { .. } => self.exec_filter(graph, op, budget, outer_rows, stats),
            Operator::Expand { .. } => self.exec_expand(graph, op, budget, outer_rows, stats),
            Operator::Project { .. } => self.exec_project(graph, op, budget, outer_rows, stats),
            Operator::Distinct { .. } => self.exec_distinct(graph, op, budget, outer_rows, stats),
            Operator::Sort { .. } => self.exec_sort(graph, op, budget, outer_rows, stats),
            Operator::Limit { .. } => self.exec_limit(graph, op, budget, outer_rows, stats),
            Operator::Skip { .. } => self.exec_skip(graph, op, budget, outer_rows, stats),
            Operator::Aggregate { .. } => self.exec_aggregate(graph, op, budget, outer_rows, stats),
            Operator::Create { .. } => self.exec_create(graph, op, budget, outer_rows, stats),
            Operator::AssemblePath { .. } => {
                self.exec_assemble_path(graph, op, budget, outer_rows, stats)
            }
            Operator::Call { .. } => self.exec_call(graph, op, budget, outer_rows, stats),
            Operator::ProduceResults { .. } => {
                self.exec_produce_results(graph, op, budget, outer_rows, stats)
            }
            Operator::VarLenExpand { .. } => {
                self.exec_var_len_expand(graph, op, budget, outer_rows, stats)
            }
            Operator::Set { .. } => self.exec_set(graph, op, budget, outer_rows, stats),
            Operator::Remove { .. } => self.exec_remove(graph, op, budget, outer_rows, stats),
            Operator::Delete { .. } => self.exec_delete(graph, op, budget, outer_rows, stats),
            Operator::Merge { .. } => self.exec_merge(graph, op, budget, outer_rows, stats),
            Operator::MergeRelationship { .. } => {
                self.exec_merge_rel(graph, op, budget, outer_rows, stats)
            }
            Operator::Unwind { .. } => self.exec_unwind(graph, op, budget, outer_rows, stats),
            Operator::Union { .. } => self.exec_union(graph, op, budget, outer_rows, stats),
            Operator::LeftOuterApply { .. } => {
                self.exec_left_outer_apply(graph, op, budget, outer_rows, stats)
            }
            Operator::Apply { .. } => self.exec_apply(graph, op, budget, outer_rows, stats),
            Operator::IndexScan { .. } => {
                self.exec_index_scan(graph, op, budget, outer_rows, stats)
            }
            Operator::NodeByIdSeek { .. } => {
                self.exec_node_by_id_seek(graph, op, budget, outer_rows, stats)
            }
            Operator::ShortestPath { .. } => {
                self.exec_shortest_path(graph, op, budget, outer_rows, stats)
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, Identifier, Label};
    use crate::executor::budget::ExecutionBudget;
    use crate::planner::{AggFunction, Operator};
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
}
