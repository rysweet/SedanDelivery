//! Cypher query parsing, planning, and execution.
//!
//! These functions adapt the Bolt RUN message to the cypher-core `QueryExecutor`,
//! handling auto-creation of graphs, plan caching, and `tx.getMetaData` exposure.

use std::collections::HashMap;
use std::time::Duration;

use cypher_core::error::GqlError;
use cypher_core::executor::budget::{configured_max_rows, OperatorProfileStats};
use cypher_core::executor::{ExecutionBudget, QueryExecutor, QueryResult};
use cypher_core::parser::parse_query;
use cypher_core::plan_cache;
use cypher_core::planner::QueryPlanner;
use cypher_core::procedure::{ProcedureDefinition, ProcedureParam, ProcedureRegistry};
use cypher_core::storage::GraphStore;
use cypher_core::types::Value;
use indexmap::IndexMap;

use crate::bridge::ensure_graph_exists;

/// Build a `ProcedureRegistry` pre-loaded with `tx.getMetaData`, which exposes
/// the transaction metadata map injected into params as `__tx_metadata__`.
fn registry_with_tx_metadata(params: &HashMap<String, Value>) -> ProcedureRegistry {
    let mut registry = ProcedureRegistry::default();
    let tx_meta = params
        .get("__tx_metadata__")
        .cloned()
        .unwrap_or(Value::Map(IndexMap::new()));
    registry.register(ProcedureDefinition {
        name: "tx.getMetaData".to_string(),
        input_params: vec![],
        output_columns: vec![ProcedureParam {
            name: "metadata".to_string(),
            type_name: "ANY?".to_string(),
        }],
        data: vec![vec![tx_meta]],
    });
    registry
}

/// Look up a cached plan for `query`, otherwise parse + plan and cache it.
fn plan_or_cached(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
    registry: &ProcedureRegistry,
    params: &HashMap<String, Value>,
) -> Result<cypher_core::planner::Operator, GqlError> {
    let cache = plan_cache::global();
    if let Some((_cached_ast, cached_plan)) = cache.get(query) {
        metrics::counter!("plan_cache_hits_total").increment(1);
        Ok(cached_plan)
    } else {
        metrics::counter!("plan_cache_misses_total").increment(1);
        let ast = parse_query(query)?;
        let planner = QueryPlanner::with_context(store, graph, registry, params);
        let plan = planner.plan(&ast)?;
        cache.insert(query, ast, plan.clone());
        Ok(plan)
    }
}

/// Build an `ExecutionBudget` from an optional Bolt `tx_timeout` (milliseconds).
fn make_budget(timeout_ms: Option<u64>) -> ExecutionBudget {
    let max_rows = configured_max_rows();
    match timeout_ms {
        Some(ms) => ExecutionBudget::new(Duration::from_millis(ms), max_rows),
        None => ExecutionBudget::new(
            Duration::from_millis(cypher_core::executor::budget::DEFAULT_TIMEOUT_MS),
            max_rows,
        ),
    }
}

/// Execute a Cypher query against the given graph in the store.
///
/// Any graph that does not yet exist is auto-created on first use.
///
/// If `timeout_ms` is `Some`, uses that value as the query timeout in
/// milliseconds (from the Bolt `tx_timeout` field). Otherwise uses the
/// default budget.
pub fn execute_query(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
    params: HashMap<String, Value>,
    timeout_ms: Option<u64>,
) -> Result<QueryResult, GqlError> {
    // Auto-create any graph that does not yet exist.
    // sedandelivery-bolt is a standalone server with its own InMemoryGraphStore,
    // so auto-creating graphs on first use is the expected behavior —
    // there is no external catalog to consult.
    ensure_graph_exists(store, graph)?;

    let registry = registry_with_tx_metadata(&params);
    let plan = plan_or_cached(store, graph, query, &registry, &params)?;

    let executor = QueryExecutor::with_context(store, &registry, params);
    let mut budget = make_budget(timeout_ms);
    executor.execute(graph, &plan, &mut budget)
}

/// Execute a Cypher query with profiling enabled, returning the result
/// and per-operator profile statistics.
///
/// The caller must also pass a `plan_annotator` closure that receives the
/// plan and profile stats while the plan is still at the same memory address
/// used during execution (important because stats are keyed by pointer address).
///
/// Returns `(QueryResult, T)` where `T` is the annotator's return value.
pub fn execute_query_with_profiling<T>(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
    params: HashMap<String, Value>,
    timeout_ms: Option<u64>,
    plan_annotator: impl FnOnce(
        &cypher_core::planner::Operator,
        Option<&HashMap<usize, OperatorProfileStats>>,
    ) -> T,
) -> Result<(QueryResult, T), GqlError> {
    ensure_graph_exists(store, graph)?;

    let registry = registry_with_tx_metadata(&params);
    let plan = plan_or_cached(store, graph, query, &registry, &params)?;

    let executor = QueryExecutor::with_context(store, &registry, params);
    let mut budget = make_budget(timeout_ms);
    budget.enable_profiling();
    let result = executor.execute(graph, &plan, &mut budget)?;
    let profile_stats = budget.take_profile_stats();

    // Annotate the plan while it's still at the same address used during execution.
    let annotation = plan_annotator(&plan, profile_stats.as_ref());

    Ok((result, annotation))
}

/// Parse and plan a Cypher query without executing it. Returns the Operator tree.
///
/// Used for EXPLAIN support: generates the query plan to return to the client.
pub fn plan_query(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
    params: HashMap<String, Value>,
) -> Result<cypher_core::planner::Operator, GqlError> {
    ensure_graph_exists(store, graph)?;

    let ast = parse_query(query)?;
    let registry = ProcedureRegistry::default();
    let planner = QueryPlanner::with_context(store, graph, &registry, &params);
    planner.plan(&ast)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cypher_core::storage::InMemoryGraphStore;

    #[test]
    fn execute_query_auto_creates_default_graph() {
        let store = InMemoryGraphStore::new();
        assert!(!store.graph_exists("default").unwrap());

        let result =
            execute_query(&store, "default", "RETURN 1 AS x", HashMap::new(), None).unwrap();
        assert_eq!(result.columns, vec!["x".to_string()]);
        assert_eq!(result.rows.len(), 1);

        assert!(store.graph_exists("default").unwrap());
    }

    #[test]
    fn execute_query_auto_creates_non_default_graph() {
        let store = InMemoryGraphStore::new();
        // Any graph should be auto-created on first use.
        let result = execute_query(&store, "mydb", "RETURN 1 AS x", HashMap::new(), None);
        assert!(
            result.is_ok(),
            "expected auto-creation of non-default graph"
        );
        assert!(store.graph_exists("mydb").unwrap());
    }

    #[test]
    fn execute_query_simple_return() {
        let store = InMemoryGraphStore::new();
        let result = execute_query(
            &store,
            "default",
            "RETURN 42 AS answer",
            HashMap::new(),
            None,
        )
        .unwrap();
        assert_eq!(result.columns, vec!["answer".to_string()]);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0]["answer"], Value::Int(42));
    }

    #[test]
    fn execute_with_params() {
        let store = InMemoryGraphStore::new();
        let mut params = HashMap::new();
        params.insert("x".to_string(), Value::Int(99));

        let result = execute_query(&store, "default", "RETURN $x AS val", params, None).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0]["val"], Value::Int(99));
    }

    #[test]
    fn streaming_error_division_by_zero_returns_partial_rows() {
        let store = InMemoryGraphStore::new();
        let result = execute_query(
            &store,
            "default",
            "UNWIND [1, 0, 2] AS x RETURN 10 / x",
            HashMap::new(),
            None,
        );
        match result {
            Ok(qr) => {
                assert_eq!(qr.rows.len(), 1, "expected 1 partial row");
                assert!(
                    qr.trailing_error.is_some(),
                    "expected trailing_error for division by zero"
                );
                let err_msg = qr.trailing_error.unwrap().to_string();
                assert!(
                    err_msg.contains("division by zero"),
                    "error should mention division by zero, got: {err_msg}"
                );
            }
            Err(e) => {
                panic!("expected Ok with partial results, got Err: {e}");
            }
        }
    }
}
