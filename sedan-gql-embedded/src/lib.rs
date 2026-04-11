//! Embeddable GQL/Cypher query engine backed by [`InMemoryGraphStore`].
//!
//! No Redis, no Bolt — just GQL parse → plan → execute in-process.
//!
//! ```
//! use sedan_gql_embedded::{SedanGql, GraphStore, InMemoryGraphStore, Value};
//!
//! let engine = SedanGql::new();
//! let store = InMemoryGraphStore::default();
//! store.create_graph("g").unwrap();
//! let result = engine
//!     .execute(&store, "g", "INSERT (n:Person {name: 'Alice'}) RETURN n.name")
//!     .unwrap();
//! assert_eq!(result.rows.len(), 1);
//! ```

pub use gql_engine::{
    GqlError, GraphStore, InMemoryGraphStore, MutationStats, QueryResult, Row, Value,
};

/// A GQL/Cypher query engine that operates entirely in-process.
pub struct SedanGql {
    _priv: (),
}

impl SedanGql {
    /// Create a new engine instance.
    pub fn new() -> Self {
        Self { _priv: () }
    }

    /// Parse, plan, and execute a Cypher/GQL query against `store`.
    ///
    /// `graph` is the name of the graph inside the store (must already exist
    /// via [`GraphStore::create_graph`]).
    pub fn execute(
        &self,
        store: &dyn GraphStore,
        graph: &str,
        query: &str,
    ) -> Result<QueryResult, GqlError> {
        gql_engine::execute(query, store, graph)
    }
}

impl Default for SedanGql {
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

    #[test]
    fn return_literal() {
        let engine = SedanGql::new();
        let store = test_store();
        let result = engine
            .execute(&store, "test", "RETURN 1 + 2 AS answer")
            .unwrap();
        assert_eq!(result.columns, vec!["answer"]);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].get("answer"), Some(&Value::Int(3)));
    }

    #[test]
    fn create_and_match() {
        let engine = SedanGql::new();
        let store = test_store();

        // Create a node via Cypher
        store
            .create_node(
                "test",
                vec!["Person".into()],
                IndexMap::from([("name".into(), Value::Str("Alice".into()))]),
            )
            .unwrap();

        let result = engine
            .execute(&store, "test", "MATCH (n:Person) RETURN n.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].get("n.name"),
            Some(&Value::Str("Alice".into()))
        );
    }

    #[test]
    fn invalid_cypher_returns_error() {
        let engine = SedanGql::new();
        let store = test_store();
        assert!(engine.execute(&store, "test", "NOT VALID !!!").is_err());
    }

    #[test]
    fn create_via_cypher_and_match() {
        let engine = SedanGql::new();
        let store = test_store();

        engine
            .execute(
                &store,
                "test",
                "CREATE (n:Movie {title: 'The Matrix'}) RETURN n.title",
            )
            .unwrap();

        let result = engine
            .execute(&store, "test", "MATCH (m:Movie) RETURN m.title")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].get("m.title"),
            Some(&Value::Str("The Matrix".into()))
        );
    }

    #[test]
    fn gql_only_keywords_work_via_public_api() {
        let engine = SedanGql::new();
        let store = test_store();

        let inserted = engine
            .execute(
                &store,
                "test",
                "INSERT (n:Movie {title: 'Arrival', year: 2016}) RETURN n.title",
            )
            .unwrap();
        assert_eq!(
            inserted.rows[0].get("n.title"),
            Some(&Value::Str("Arrival".into()))
        );

        let filtered = engine
            .execute(
                &store,
                "test",
                "MATCH (n:Movie) FILTER n.year >= 2016 RETURN n.title",
            )
            .unwrap();
        assert_eq!(filtered.rows.len(), 1);
    }
}
