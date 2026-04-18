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

#![warn(missing_docs)]

use std::collections::HashMap;

pub use gql_engine::{
    GqlError, GraphStore, InMemoryGraphStore, MutationStats, QueryResult, Row, Value,
};

/// Re-export `ProcedureRegistry` so users of `execute_with_context` don't need
/// a direct `cypher-core` dependency.
pub use cypher_core::procedure::ProcedureRegistry;

/// Convenience prelude — `use sedan_gql_embedded::prelude::*` imports the types
/// needed for most use-cases.
pub mod prelude {
    pub use crate::{
        GqlError, GraphStore, InMemoryGraphStore, MutationStats, ProcedureRegistry, QueryResult,
        Row, SedanGql, Value,
    };
}

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

    /// Execute a query with parameters and a procedure registry.
    ///
    /// This is the parameterised variant — use it when you need `$param`
    /// placeholders or custom stored procedures.
    ///
    /// ```
    /// use std::collections::HashMap;
    /// use sedan_gql_embedded::prelude::*;
    ///
    /// let engine = SedanGql::new();
    /// let store = InMemoryGraphStore::default();
    /// store.create_graph("g").unwrap();
    ///
    /// let mut params = HashMap::new();
    /// params.insert("name".into(), Value::Str("Alice".into()));
    ///
    /// let result = engine
    ///     .execute_with_context(
    ///         &store,
    ///         "g",
    ///         "INSERT (n:Person {name: $name}) RETURN n.name",
    ///         &ProcedureRegistry::default(),
    ///         params,
    ///     )
    ///     .unwrap();
    /// assert_eq!(result.rows[0].get("n.name"), Some(&Value::Str("Alice".into())));
    /// ```
    pub fn execute_with_context(
        &self,
        store: &dyn GraphStore,
        graph: &str,
        query: &str,
        registry: &ProcedureRegistry,
        parameters: HashMap<String, Value>,
    ) -> Result<QueryResult, GqlError> {
        gql_engine::execute_with_context(query, store, graph, registry, parameters)
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

    fn test_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::default();
        store.create_graph("test").expect("create graph");
        store
    }

    /// Smoke test: basic execute round-trip.
    /// Full coverage is in tests/outside_in.rs, tests/parity.rs, etc.
    #[test]
    fn execute_smoke() {
        let engine = SedanGql::new();
        let store = test_store();
        let qr = engine
            .execute(&store, "test", "RETURN 1 + 2 AS answer")
            .unwrap();
        assert_eq!(qr.rows[0].get("answer"), Some(&Value::Int(3)));
    }

    /// Verifies the prelude re-exports all essential types (compile-time check).
    #[test]
    fn prelude_reexports_all_essential_types() {
        use crate::prelude::*;

        let engine = SedanGql::new();
        let store = InMemoryGraphStore::default();
        store.create_graph("p").expect("create graph");

        let _registry = ProcedureRegistry::default();
        let result: Result<QueryResult, GqlError> = engine.execute(&store, "p", "RETURN 1 AS x");
        let qr = result.unwrap();
        let _row: &Row = &qr.rows[0];
        let _val: &Value = _row.get("x").unwrap();
        let _stats: MutationStats = qr.stats;
    }
}
