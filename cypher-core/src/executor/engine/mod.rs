//! Query executor — evaluates a physical [`Operator`] tree against a [`GraphStore`].
//!
//! A [`Row`] is an owned map from column name to [`Value`].  During execution
//! node/edge variables hold `Value::Node(id)` / `Value::Edge(id)`.  When an
//! expression needs a property value (e.g. `n.name`) the executor resolves the
//! ID against the store and returns the concrete property value.
//!
//! Every operator checks `budget.check_or_err()` before doing any work.

mod aggregation;
mod cache;
mod decode;
mod dispatch;
mod eval;
pub(crate) mod helpers;
mod joins;
mod mutations;
mod produce;
mod scan;
mod scan_helpers;
mod shaping;
mod types;

mod gremlin;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use crate::error::GqlError;
use crate::executor::budget::ExecutionBudget;
use crate::planner::Operator;
use crate::procedure::ProcedureRegistry;
use crate::storage::{EdgeData, EdgeId, GraphStore, NodeData, NodeId};
use crate::types::Value;

pub(crate) use decode::BfsArena;
pub use helpers::cmp_values;
pub(crate) use types::ExecContext;
pub use types::{MutationStats, QueryResult, Row};

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
    /// Gremlin side-effect store: holds values set by side-effect steps
    /// (group("a"), groupCount("a"), aggregate("a")) so that cap() can
    /// retrieve them even when downstream steps produce zero traversers.
    pub(crate) gremlin_side_effects: std::cell::RefCell<HashMap<String, Value>>,
    /// Shared dedup state for `Distinct` operators inside `Repeat`.
    /// When set, `exec_distinct` uses (and updates) this set instead of
    /// creating a fresh one, giving cross-iteration dedup semantics.
    pub(crate) repeat_dedup_state: std::cell::RefCell<Option<std::collections::HashSet<String>>>,
    /// Global property ID counter for Gremlin `properties()` step.
    pub(crate) property_id_counter: std::cell::Cell<i64>,
    /// Stable per-traversal property ID interning so repeated logical
    /// properties reuse the same synthetic id.
    pub(crate) property_id_map: std::cell::RefCell<HashMap<String, i64>>,
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
            gremlin_side_effects: std::cell::RefCell::new(HashMap::new()),
            repeat_dedup_state: std::cell::RefCell::new(None),
            property_id_counter: std::cell::Cell::new(0),
            property_id_map: std::cell::RefCell::new(HashMap::new()),
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
            gremlin_side_effects: std::cell::RefCell::new(HashMap::new()),
            repeat_dedup_state: std::cell::RefCell::new(None),
            property_id_counter: std::cell::Cell::new(0),
            property_id_map: std::cell::RefCell::new(HashMap::new()),
        }
    }

    /// Enable Gremlin mode: `Project` operators preserve all input columns
    /// so that step labels set via `as()` survive property projections.
    pub fn with_gremlin_mode(mut self) -> Self {
        self.gremlin_mode = true;
        self
    }

    /// Extract a sort key from a value, resolving node/edge properties via graph store.
    fn extract_sort_key_with_graph(&self, graph: &str, v: &Value, key: &str) -> Value {
        match v {
            Value::Map(m) => m.get(key).cloned().unwrap_or(Value::Null),
            Value::Node(nid) => {
                if let Ok(Some(data)) = self.cached_get_node(graph, *nid) {
                    data.properties.get(key).cloned().unwrap_or(Value::Null)
                } else {
                    Value::Null
                }
            }
            Value::Edge(eid) => {
                if let Ok(Some(data)) = self.cached_get_edge(graph, *eid) {
                    data.properties.get(key).cloned().unwrap_or(Value::Null)
                } else {
                    Value::Null
                }
            }
            _ => Value::Null,
        }
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
}
