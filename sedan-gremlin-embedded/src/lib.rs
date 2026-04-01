//! Embeddable Gremlin query engine backed by [`InMemoryGraphStore`].
//!
//! Parses a Gremlin traversal string, compiles it to an operator tree via
//! [`gremlin_core::GremlinCompiler`], and executes it with the cypher-core
//! [`QueryExecutor`] in Gremlin mode.
//!
//! ```
//! use sedan_gremlin_embedded::{GremlinEngine, GraphStore, InMemoryGraphStore};
//!
//! let engine = GremlinEngine::new();
//! let store = InMemoryGraphStore::default();
//! store.create_graph("g").unwrap();
//! ```

pub use cypher_core::error::GqlError;
pub use cypher_core::executor::{MutationStats, QueryResult, Row};
pub use cypher_core::storage::{GraphStore, InMemoryGraphStore};
pub use cypher_core::types::Value;

use cypher_core::executor::{ExecutionBudget, QueryExecutor};
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

/// A Gremlin query engine that operates entirely in-process.
pub struct GremlinEngine {
    _priv: (),
}

impl GremlinEngine {
    /// Create a new engine instance.
    pub fn new() -> Self {
        Self { _priv: () }
    }

    /// Parse, compile, and execute a Gremlin traversal against `store`.
    ///
    /// `graph` is the name of the graph inside the store (must already exist).
    /// `gremlin_query` should be a traversal string such as
    /// `"g.V().has('name','Alice').values('name')"`.
    pub fn execute(
        &self,
        store: &dyn GraphStore,
        graph: &str,
        gremlin_query: &str,
    ) -> Result<QueryResult, GremlinError> {
        let no_params = |_: &str| -> Option<GremlinValue> { None };
        let steps = parse_traversal(gremlin_query, &no_params)?;
        let mut compiler = GremlinCompiler::new();
        let operator = compiler.compile(&steps)?;
        let mut budget = ExecutionBudget::default_budget();
        let result =
            QueryExecutor::new(store)
                .with_gremlin_mode()
                .execute(graph, &operator, &mut budget)?;
        Ok(result)
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
        use cypher_core::storage::GraphStore;

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
}
