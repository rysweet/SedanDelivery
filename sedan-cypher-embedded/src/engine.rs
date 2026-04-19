//! The [`CypherEngine`] facade and its query-execution methods.
//!
//! This module is private; the public surface is re-exported from the
//! crate root.

use std::collections::HashMap;

use cypher_core::error::GqlError;
use cypher_core::executor::{ExecutionBudget, QueryExecutor, QueryResult};
use cypher_core::parser::parse_query;
use cypher_core::plan_cache::PlanCache;
use cypher_core::planner::QueryPlanner;
use cypher_core::procedure::ProcedureRegistry;
use cypher_core::storage::GraphStore;
use cypher_core::types::Value;

/// Default plan cache capacity for embedded use.
const DEFAULT_CACHE_CAPACITY: usize = 128;

/// An openCypher query engine that operates entirely in-process.
///
/// Wraps [`cypher_core`]'s parser, planner, and executor with an internal
/// plan cache so repeated queries skip parsing and planning. The cache is
/// keyed by query string and uses epoch-based eviction when full. Call
/// [`CypherEngine::clear_cache`] after schema-changing DDL operations
/// (e.g. `CREATE INDEX`) that could invalidate cached plans.
///
/// `CypherEngine` is cheap to construct and `Send + Sync`-friendly when
/// shared behind an `Arc`. Pair one engine with one or more
/// [`GraphStore`](cypher_core::storage::GraphStore) implementations.
///
/// ```
/// use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};
///
/// let engine = CypherEngine::new();
/// let store = InMemoryGraphStore::default();
/// store.create_graph("g").unwrap();
/// let r = engine.execute(&store, "g", "RETURN 1 AS n").unwrap();
/// assert_eq!(r.rows[0]["n"], Value::Int(1));
/// ```
pub struct CypherEngine {
    cache: PlanCache,
}

impl CypherEngine {
    /// Create a new engine with the default plan cache capacity (128 plans).
    ///
    /// ```
    /// # use sedan_cypher_embedded::CypherEngine;
    /// let _engine = CypherEngine::new();
    /// ```
    pub fn new() -> Self {
        Self {
            cache: PlanCache::new(DEFAULT_CACHE_CAPACITY),
        }
    }

    /// Create a new engine with a specific plan cache capacity.
    ///
    /// Pass `0` to disable caching entirely — useful in tests that want to
    /// force re-planning on every call.
    ///
    /// ```
    /// # use sedan_cypher_embedded::CypherEngine;
    /// let _no_cache = CypherEngine::with_cache_capacity(0);
    /// let _bigger = CypherEngine::with_cache_capacity(1024);
    /// ```
    pub fn with_cache_capacity(capacity: usize) -> Self {
        Self {
            cache: PlanCache::new(capacity),
        }
    }

    /// Parse, plan, and execute an openCypher `query` against `store` in
    /// the named `graph`.
    ///
    /// On a cache hit, parsing and planning are skipped; the cached plan
    /// is re-executed against the current store state, so writes (CREATE,
    /// SET, DELETE, MERGE, REMOVE) always observe fresh data. Call
    /// [`CypherEngine::clear_cache`] after DDL that could invalidate plans
    /// (e.g. `CREATE INDEX`).
    ///
    /// For queries that need parameters (`$name`) or user-defined
    /// procedures, use [`CypherEngine::execute_with_parameters`].
    ///
    /// ```
    /// use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};
    ///
    /// let engine = CypherEngine::new();
    /// let store = InMemoryGraphStore::default();
    /// store.create_graph("g").unwrap();
    /// let r = engine.execute(&store, "g", "RETURN 'hi' AS msg").unwrap();
    /// assert_eq!(r.rows[0]["msg"], Value::Str("hi".into()));
    /// ```
    pub fn execute(
        &self,
        store: &dyn GraphStore,
        graph: &str,
        query: &str,
    ) -> Result<QueryResult, GqlError> {
        let (ast, plan) = if let Some(cached) = self.cache.get(query) {
            cached
        } else {
            let ast = parse_query(query)?;
            let planner = QueryPlanner::new(store, graph);
            let plan = planner.plan(&ast)?;
            self.cache.insert(query, ast.clone(), plan.clone());
            (ast, plan)
        };
        let _ = ast; // AST is retained in the cache but not needed for execution
        let mut budget = ExecutionBudget::default_budget();
        QueryExecutor::new(store).execute(graph, &plan, &mut budget)
    }

    /// Clear the plan cache.
    ///
    /// Call this after schema-changing DDL operations (e.g. `CREATE INDEX`)
    /// or whenever cached plans may no longer be valid.
    ///
    /// ```
    /// # use sedan_cypher_embedded::CypherEngine;
    /// let engine = CypherEngine::new();
    /// engine.clear_cache();
    /// ```
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Parse, plan, and execute an openCypher query with caller-supplied
    /// parameters and an optional procedure registry.
    ///
    /// This is the full-featured execute path used by the openCypher TCK.
    /// For ordinary queries without parameters or stored procedures, prefer
    /// [`CypherEngine::execute`].
    ///
    /// `parameters` are referenced from the query as `$name`. `registry`
    /// supplies any user-defined procedures the query may `CALL`. Pass
    /// [`ProcedureRegistry::new`] for an empty registry.
    ///
    /// Plans produced by this path are **not** cached — parameter values
    /// can influence plan selection (e.g. via cardinality hints), so
    /// caching them by query string alone would be unsound.
    ///
    /// ```
    /// use std::collections::HashMap;
    /// use sedan_cypher_embedded::{
    ///     CypherEngine, GraphStore, InMemoryGraphStore, ProcedureRegistry, Value,
    /// };
    ///
    /// let engine = CypherEngine::new();
    /// let store = InMemoryGraphStore::default();
    /// store.create_graph("g").unwrap();
    ///
    /// let mut params = HashMap::new();
    /// params.insert("threshold".to_string(), Value::Int(10));
    /// let registry = ProcedureRegistry::new();
    ///
    /// let result = engine
    ///     .execute_with_parameters(&store, "g", "RETURN $threshold AS t", &params, &registry)
    ///     .unwrap();
    /// assert_eq!(result.rows[0]["t"], Value::Int(10));
    /// ```
    pub fn execute_with_parameters(
        &self,
        store: &dyn GraphStore,
        graph: &str,
        query: &str,
        parameters: &HashMap<String, Value>,
        registry: &ProcedureRegistry,
    ) -> Result<QueryResult, GqlError> {
        let ast = parse_query(query)?;
        let planner = QueryPlanner::with_context(store, graph, registry, parameters);
        let plan = planner.plan(&ast)?;
        let mut budget = ExecutionBudget::default_budget();
        QueryExecutor::with_context(store, registry, parameters.clone()).execute(
            graph,
            &plan,
            &mut budget,
        )
    }
}

impl Default for CypherEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
