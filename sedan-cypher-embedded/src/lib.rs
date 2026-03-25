//! Embeddable openCypher query engine.
//!
//! This is a thin wrapper around [`sedan_gql_embedded`] exposing an
//! openCypher-branded API.  The underlying engine is identical.
//!
//! ```
//! use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};
//!
//! let engine = CypherEngine::new();
//! let store = InMemoryGraphStore::default();
//! store.create_graph("g").unwrap();
//! let result = engine.execute(&store, "g", "RETURN 'hello' AS msg").unwrap();
//! assert_eq!(result.rows.len(), 1);
//! ```

pub use sedan_gql_embedded::{
    GqlError, GraphStore, InMemoryGraphStore, MutationStats, QueryResult, Row, Value,
};

use sedan_gql_embedded::SedanGql;

/// An openCypher query engine that operates entirely in-process.
pub struct CypherEngine {
    inner: SedanGql,
}

impl CypherEngine {
    /// Create a new engine instance.
    pub fn new() -> Self {
        Self {
            inner: SedanGql::new(),
        }
    }

    /// Parse, plan, and execute an openCypher query against `store`.
    pub fn execute(
        &self,
        store: &dyn GraphStore,
        graph: &str,
        query: &str,
    ) -> Result<QueryResult, GqlError> {
        self.inner.execute(store, graph, query)
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
