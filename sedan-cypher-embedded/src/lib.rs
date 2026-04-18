//! Embeddable openCypher query engine.
//!
//! This crate exposes the shared [`cypher-core`] parser, planner, and executor as
//! an in-process openCypher API through a single [`CypherEngine`] struct.
//!
//! # Getting started
//!
//! ```
//! use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};
//!
//! let engine = CypherEngine::new();
//! let store = InMemoryGraphStore::default();
//! store.create_graph("g").unwrap();
//! let result = engine.execute(&store, "g", "RETURN 'hello' AS msg").unwrap();
//! assert_eq!(result.rows[0]["msg"], Value::Str("hello".into()));
//! ```
//!
//! # Create, match, and aggregate
//!
//! ```
//! use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};
//!
//! let engine = CypherEngine::new();
//! let store = InMemoryGraphStore::default();
//! store.create_graph("g").unwrap();
//!
//! // Create nodes
//! engine.execute(&store, "g",
//!     "CREATE (:Person {name: 'Alice', age: 30}), (:Person {name: 'Bob', age: 25})"
//! ).unwrap();
//!
//! // Aggregate
//! let result = engine.execute(&store, "g",
//!     "MATCH (p:Person) RETURN count(p) AS n, avg(p.age) AS avg_age"
//! ).unwrap();
//! assert_eq!(result.rows[0]["n"], Value::Int(2));
//! ```
//!
//! # Error handling
//!
//! ```
//! use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, GqlError};
//!
//! let engine = CypherEngine::new();
//! let store = InMemoryGraphStore::default();
//! store.create_graph("g").unwrap();
//!
//! match engine.execute(&store, "g", "NOT VALID CYPHER") {
//!     Err(GqlError::Parse(e)) => { /* syntax error */ }
//!     Err(other) => { /* plan, execution, storage, or type error */ }
//!     Ok(_) => unreachable!(),
//! }
//! ```

#![warn(missing_docs)]

pub use cypher_core::error::GqlError;
pub use cypher_core::executor::{MutationStats, QueryResult, Row};
pub use cypher_core::procedure::ProcedureRegistry;
pub use cypher_core::storage::{GraphStore, InMemoryGraphStore};
pub use cypher_core::types::Value;

use std::collections::HashMap;

use cypher_core::executor::{ExecutionBudget, QueryExecutor};
use cypher_core::parser::parse_query;
use cypher_core::plan_cache::PlanCache;
use cypher_core::planner::QueryPlanner;

/// Default plan cache capacity for embedded use.
const DEFAULT_CACHE_CAPACITY: usize = 128;

/// An openCypher query engine that operates entirely in-process.
///
/// Maintains an internal plan cache so that repeated queries skip parsing
/// and planning. The cache is automatically invalidated when it reaches
/// capacity (epoch-based eviction). Call [`CypherEngine::clear_cache`]
/// after schema-changing DDL operations.
pub struct CypherEngine {
    cache: PlanCache,
}

impl CypherEngine {
    /// Create a new engine instance with the default cache capacity (128 plans).
    pub fn new() -> Self {
        Self {
            cache: PlanCache::new(DEFAULT_CACHE_CAPACITY),
        }
    }

    /// Create a new engine with a specific plan cache capacity.
    ///
    /// Use `0` to disable caching entirely.
    pub fn with_cache_capacity(capacity: usize) -> Self {
        Self {
            cache: PlanCache::new(capacity),
        }
    }

    /// Parse, plan, and execute an openCypher query against `store`.
    ///
    /// On cache hit the parsing and planning steps are skipped. Cached plans
    /// are executed against the current store state, so write queries
    /// (CREATE, SET, DELETE, MERGE, REMOVE) automatically observe fresh data
    /// even when the plan is reused. Call [`CypherEngine::clear_cache`] after
    /// schema-changing DDL operations (e.g. `CREATE INDEX`) that could
    /// invalidate cached plans.
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
    /// or when the graph structure changes in ways that could invalidate
    /// cached plans.
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Parse, plan, and execute an openCypher query against `store`, with
    /// caller-supplied parameters and an optional procedure registry.
    ///
    /// This is the full-featured execute path used by the openCypher TCK.
    /// For ordinary use without parameters or stored procedures, prefer
    /// [`CypherEngine::execute`].
    ///
    /// `parameters` are referenced from the query as `$name`. `registry`
    /// supplies any user-defined procedures the query may `CALL`. Pass
    /// [`ProcedureRegistry::new`] for the empty registry.
    ///
    /// Plans produced by this path are NOT cached — parameter values can
    /// influence plan selection (e.g. via cardinality hints), so caching
    /// them by query string alone would be unsound.
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
mod tests {
    use super::*;

    fn test_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::default();
        store.create_graph("test").expect("create graph");
        store
    }

    #[test]
    fn return_literal() {
        let engine = CypherEngine::new();
        let store = test_store();
        let result = engine.execute(&store, "test", "RETURN 42 AS n").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].get("n"), Some(&Value::Int(42)));
    }

    #[test]
    fn create_and_match() {
        let engine = CypherEngine::new();
        let store = test_store();

        engine
            .execute(
                &store,
                "test",
                "CREATE (p:Person {name: 'Bob'}) RETURN p.name",
            )
            .unwrap();

        let result = engine
            .execute(&store, "test", "MATCH (p:Person) RETURN p.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].get("p.name"),
            Some(&Value::Str("Bob".into()))
        );
    }

    #[test]
    fn invalid_cypher_returns_error() {
        let engine = CypherEngine::new();
        let store = test_store();
        assert!(engine.execute(&store, "test", "GARBAGE!!!").is_err());
    }
}
