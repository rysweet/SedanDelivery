//! Embeddable, in-process Gremlin query engine backed by [`InMemoryGraphStore`].
//!
//! Parses a Gremlin traversal string, compiles it to an operator tree via
//! [`gremlin_core::GremlinCompiler`], and executes it with the cypher-core
//! `QueryExecutor` in Gremlin mode. No server, no network, no configuration.
//!
//! # Quick start
//!
//! ```
//! use sedan_gremlin_embedded::{GremlinEngine, GraphStore, InMemoryGraphStore, Value};
//! use indexmap::IndexMap;
//!
//! let engine = GremlinEngine::new();
//! let store = InMemoryGraphStore::default();
//! store.create_graph("g").unwrap();
//!
//! // Add a vertex
//! store.create_node(
//!     "g",
//!     vec!["Person".into()],
//!     IndexMap::from([("name".into(), Value::Str("Alice".into()))]),
//! ).unwrap();
//!
//! // Query with Gremlin
//! let result = engine.execute(&store, "g", "g.V().count()").unwrap();
//! assert_eq!(result.rows.len(), 1);
//! ```
//!
//! # Error handling
//!
//! [`GremlinError`] distinguishes parse, compile, and execution failures:
//!
//! ```
//! use sedan_gremlin_embedded::{GremlinEngine, GremlinError, GraphStore, InMemoryGraphStore};
//!
//! let engine = GremlinEngine::new();
//! let store = InMemoryGraphStore::default();
//! store.create_graph("g").unwrap();
//!
//! match engine.execute(&store, "g", "bad.query()") {
//!     Err(GremlinError::Parse(e)) => { /* syntax error */ }
//!     Err(GremlinError::Compile(e)) => { /* unsupported step */ }
//!     Err(GremlinError::Execution(e)) => { /* runtime error */ }
//!     Ok(result) => { /* success */ }
//! }
//! ```

#![warn(missing_docs)]

pub use cypher_core::error::GqlError;
pub use cypher_core::executor::{ExecutionBudget, MutationStats, QueryResult, Row};
pub use cypher_core::storage::{GraphStore, InMemoryGraphStore};
pub use cypher_core::types::Value;

/// Convenience re-exports for the most common types.
///
/// ```
/// use sedan_gremlin_embedded::prelude::*;
/// ```
pub mod prelude {
    pub use crate::{
        CompiledQuery, ExecutionBudget, GqlError, GraphStore, GremlinEngine, GremlinError,
        InMemoryGraphStore, MutationStats, QueryResult, Row, Value,
    };
}

use cypher_core::executor::QueryExecutor;
use cypher_core::planner::Operator;
use gremlin_core::types::GremlinValue;
use gremlin_core::{parse_traversal, GremlinCompiler, TextParseError};

/// Errors from the Gremlin embedded engine.
#[derive(Debug, thiserror::Error)]
pub enum GremlinError {
    /// Gremlin text parsing failed.
    #[error("parse error: {0}")]
    Parse(#[from] TextParseError),

    /// Gremlin-to-operator compilation failed.
    #[error("compile error: {0}")]
    Compile(#[from] gremlin_core::compiler::CompileError),

    /// Operator execution failed.
    #[error("execution error: {0}")]
    Execution(#[from] GqlError),
}

/// A pre-compiled Gremlin query that can be executed multiple times
/// without re-parsing or re-compiling the traversal string.
///
/// Compile once with [`GremlinEngine::compile`], then execute many times
/// with [`GremlinEngine::execute_compiled`].
///
/// ```
/// use sedan_gremlin_embedded::{GremlinEngine, GraphStore, InMemoryGraphStore};
///
/// let engine = GremlinEngine::new();
/// let store = InMemoryGraphStore::default();
/// store.create_graph("g").unwrap();
///
/// let query = engine.compile("g.V().count()").unwrap();
///
/// // Execute the same compiled query against different graphs or stores
/// let r1 = engine.execute_compiled(&store, "g", &query).unwrap();
/// let r2 = engine.execute_compiled(&store, "g", &query).unwrap();
/// ```
pub struct CompiledQuery {
    operator: Operator,
}

/// A Gremlin query engine that operates entirely in-process.
pub struct GremlinEngine {
    _priv: (),
}

impl GremlinEngine {
    /// Create a new engine instance.
    pub fn new() -> Self {
        Self { _priv: () }
    }

    /// Parse and compile a Gremlin traversal string into a reusable
    /// [`CompiledQuery`] without executing it.
    ///
    /// Use [`execute_compiled`](Self::execute_compiled) to run the result.
    pub fn compile(&self, gremlin_query: &str) -> Result<CompiledQuery, GremlinError> {
        let no_params = |_: &str| -> Option<GremlinValue> { None };
        let steps = parse_traversal(gremlin_query, &no_params)?;
        let mut compiler = GremlinCompiler::new();
        let operator = compiler.compile(&steps)?;
        Ok(CompiledQuery { operator })
    }

    /// Execute a pre-compiled query against `store`.
    ///
    /// Uses the default execution budget. For custom budgets, use
    /// [`execute_compiled_with_budget`](Self::execute_compiled_with_budget).
    pub fn execute_compiled(
        &self,
        store: &dyn GraphStore,
        graph: &str,
        query: &CompiledQuery,
    ) -> Result<QueryResult, GremlinError> {
        let mut budget = ExecutionBudget::default_budget();
        self.execute_compiled_with_budget(store, graph, query, &mut budget)
    }

    /// Execute a pre-compiled query with a caller-supplied execution budget.
    ///
    /// This gives full control over timeout and row limits.
    ///
    /// ```
    /// use sedan_gremlin_embedded::{ExecutionBudget, GremlinEngine, GraphStore, InMemoryGraphStore};
    /// use std::time::Duration;
    ///
    /// let engine = GremlinEngine::new();
    /// let store = InMemoryGraphStore::default();
    /// store.create_graph("g").unwrap();
    ///
    /// let query = engine.compile("g.V().count()").unwrap();
    /// let mut budget = ExecutionBudget::new(Duration::from_secs(5), 1_000);
    /// let result = engine.execute_compiled_with_budget(&store, "g", &query, &mut budget).unwrap();
    /// ```
    pub fn execute_compiled_with_budget(
        &self,
        store: &dyn GraphStore,
        graph: &str,
        query: &CompiledQuery,
        budget: &mut ExecutionBudget,
    ) -> Result<QueryResult, GremlinError> {
        let result = QueryExecutor::new(store).with_gremlin_mode().execute(
            graph,
            &query.operator,
            budget,
        )?;
        Ok(result)
    }

    /// Parse, compile, and execute a Gremlin traversal against `store`.
    ///
    /// `graph` is the name of the graph inside the store (must already exist).
    /// `gremlin_query` should be a traversal string such as
    /// `"g.V().has('name','Alice').values('name')"`.
    ///
    /// For repeated queries, prefer [`compile`](Self::compile) +
    /// [`execute_compiled`](Self::execute_compiled) to avoid re-parsing.
    pub fn execute(
        &self,
        store: &dyn GraphStore,
        graph: &str,
        gremlin_query: &str,
    ) -> Result<QueryResult, GremlinError> {
        let query = self.compile(gremlin_query)?;
        self.execute_compiled(store, graph, &query)
    }

    /// Execute a Gremlin traversal with a caller-supplied execution budget.
    ///
    /// Combines parsing, compiling, and execution in one call with custom
    /// timeout and row limits.
    pub fn execute_with_budget(
        &self,
        store: &dyn GraphStore,
        graph: &str,
        gremlin_query: &str,
        budget: &mut ExecutionBudget,
    ) -> Result<QueryResult, GremlinError> {
        let query = self.compile(gremlin_query)?;
        self.execute_compiled_with_budget(store, graph, &query, budget)
    }
}

impl Default for GremlinEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    fn test_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::default();
        store.create_graph("test").expect("create graph");
        store
    }

    fn seed_graph(store: &InMemoryGraphStore) {
        store
            .create_node(
                "test",
                vec!["Person".into()],
                IndexMap::from([
                    ("name".into(), Value::Str("Alice".into())),
                    ("age".into(), Value::Int(30)),
                ]),
            )
            .unwrap();

        store
            .create_node(
                "test",
                vec!["Person".into()],
                IndexMap::from([
                    ("name".into(), Value::Str("Bob".into())),
                    ("age".into(), Value::Int(25)),
                ]),
            )
            .unwrap();
    }

    #[test]
    fn vertex_count() {
        let engine = GremlinEngine::new();
        let store = test_store();
        seed_graph(&store);

        let result = engine.execute(&store, "test", "g.V().count()").unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn has_filter() {
        let engine = GremlinEngine::new();
        let store = test_store();
        seed_graph(&store);

        let result = engine
            .execute(
                &store,
                "test",
                "g.V().has(\"name\",\"Alice\").values(\"name\")",
            )
            .unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn invalid_traversal_returns_error() {
        let engine = GremlinEngine::new();
        let store = test_store();
        assert!(engine.execute(&store, "test", "not.a.traversal()").is_err());
    }

    #[test]
    fn compile_then_execute() {
        let engine = GremlinEngine::new();
        let store = test_store();
        seed_graph(&store);

        let query = engine.compile("g.V().count()").unwrap();
        let r1 = engine.execute_compiled(&store, "test", &query).unwrap();
        let r2 = engine.execute_compiled(&store, "test", &query).unwrap();
        assert_eq!(r1.rows.len(), 1);
        assert_eq!(r2.rows.len(), 1);
    }

    #[test]
    fn compile_error_propagates() {
        let engine = GremlinEngine::new();
        assert!(engine.compile("bad.query()").is_err());
    }

    #[test]
    fn execute_with_budget() {
        use std::time::Duration;

        let engine = GremlinEngine::new();
        let store = test_store();
        seed_graph(&store);

        let mut budget = ExecutionBudget::new(Duration::from_secs(5), 1_000);
        let result = engine
            .execute_with_budget(&store, "test", "g.V().count()", &mut budget)
            .unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn compiled_query_reuse_across_stores() {
        let engine = GremlinEngine::new();
        let query = engine.compile("g.V().count()").unwrap();

        let s1 = test_store();
        seed_graph(&s1);
        let s2 = test_store();

        let r1 = engine.execute_compiled(&s1, "test", &query).unwrap();
        let r2 = engine.execute_compiled(&s2, "test", &query).unwrap();

        // s1 has 2 vertices, s2 has 0
        assert_eq!(r1.rows.len(), 1);
        assert_eq!(r2.rows.len(), 1);
    }
}
