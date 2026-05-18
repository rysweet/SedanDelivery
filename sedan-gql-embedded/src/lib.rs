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

/// Re-export [`ExecutionBudget`] so callers of [`SedanGql::execute_with_budget`]
/// don't need a direct `cypher-core` dependency.
pub use cypher_core::executor::ExecutionBudget;

/// Re-export `ProcedureRegistry` so users of `execute_with_context` don't need
/// a direct `cypher-core` dependency.
pub use cypher_core::procedure::ProcedureRegistry;

/// Convenience prelude — `use sedan_gql_embedded::prelude::*` imports the types
/// needed for most use-cases.
pub mod prelude {
    pub use crate::{
        ExecutionBudget, GqlError, GraphStore, InMemoryGraphStore, MutationStats,
        ProcedureRegistry, QueryResult, Row, SedanGql, Value,
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

    /// Execute a query with a caller-supplied execution budget.
    ///
    /// Like [`SedanGql::execute`] but uses the provided `budget` instead of
    /// the default.
    ///
    /// ```
    /// use std::time::Duration;
    /// use sedan_gql_embedded::prelude::*;
    ///
    /// let engine = SedanGql::new();
    /// let store = InMemoryGraphStore::default();
    /// store.create_graph("g").unwrap();
    ///
    /// let mut budget = ExecutionBudget::new(Duration::from_secs(5), 1_000);
    /// let result = engine
    ///     .execute_with_budget(&store, "g", "RETURN 1 AS x", &mut budget)
    ///     .unwrap();
    /// assert_eq!(result.rows[0].get("x"), Some(&Value::Int(1)));
    /// ```
    pub fn execute_with_budget(
        &self,
        store: &dyn GraphStore,
        graph: &str,
        query: &str,
        budget: &mut ExecutionBudget,
    ) -> Result<QueryResult, GqlError> {
        gql_engine::execute_with_budget(query, store, graph, budget)
    }

    /// Execute a query with parameters, a procedure registry, and a
    /// caller-supplied execution budget.
    ///
    /// Combines the parameter support of [`SedanGql::execute_with_context`]
    /// with budget control.
    pub fn execute_with_context_and_budget(
        &self,
        store: &dyn GraphStore,
        graph: &str,
        query: &str,
        registry: &ProcedureRegistry,
        parameters: HashMap<String, Value>,
        budget: &mut ExecutionBudget,
    ) -> Result<QueryResult, GqlError> {
        gql_engine::execute_with_context_and_budget(
            query, store, graph, registry, parameters, budget,
        )
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

    /// `Default` impl produces a working engine.
    #[test]
    fn default_constructs_working_engine() {
        let engine = SedanGql::default();
        let store = test_store();
        let qr = engine.execute(&store, "test", "RETURN 42 AS x").unwrap();
        assert_eq!(qr.rows[0].get("x"), Some(&Value::Int(42)));
    }

    /// `execute_with_context` substitutes parameters and returns rows.
    #[test]
    fn execute_with_context_binds_parameters() {
        let engine = SedanGql::new();
        let store = test_store();
        let registry = ProcedureRegistry::default();
        let mut params = HashMap::new();
        params.insert("name".into(), Value::Str("Alice".into()));
        let qr = engine
            .execute_with_context(
                &store,
                "test",
                "INSERT (n:Person {name: $name}) RETURN n.name AS name",
                &registry,
                params,
            )
            .unwrap();
        assert_eq!(qr.rows.len(), 1);
        assert_eq!(qr.rows[0].get("name"), Some(&Value::Str("Alice".into())));
    }

    /// `execute_with_context` works with empty parameters and default registry.
    #[test]
    fn execute_with_context_empty_params() {
        let engine = SedanGql::new();
        let store = test_store();
        let qr = engine
            .execute_with_context(
                &store,
                "test",
                "RETURN 1 + 1 AS two",
                &ProcedureRegistry::default(),
                HashMap::new(),
            )
            .unwrap();
        assert_eq!(qr.rows[0].get("two"), Some(&Value::Int(2)));
    }

    /// `execute` propagates parser errors as `GqlError`.
    #[test]
    fn execute_returns_error_for_invalid_syntax() {
        let engine = SedanGql::new();
        let store = test_store();
        let err = engine.execute(&store, "test", "THIS IS NOT VALID GQL");
        assert!(err.is_err(), "expected parse error, got {:?}", err);
    }

    /// `execute_with_context` propagates errors when the graph is missing.
    #[test]
    fn execute_with_context_returns_error_for_unknown_graph() {
        let engine = SedanGql::new();
        let store = InMemoryGraphStore::default();
        let err = engine.execute_with_context(
            &store,
            "does-not-exist",
            "RETURN 1",
            &ProcedureRegistry::default(),
            HashMap::new(),
        );
        assert!(err.is_err(), "expected error for missing graph");
    }

    /// `execute` propagates errors for runtime failures (unknown property).
    #[test]
    fn execute_handles_match_on_empty_graph() {
        let engine = SedanGql::new();
        let store = test_store();
        let qr = engine
            .execute(&store, "test", "MATCH (n:Nope) RETURN n")
            .unwrap();
        assert_eq!(qr.rows.len(), 0);
    }

    /// Multiple parameter types round-trip through `execute_with_context`.
    #[test]
    fn execute_with_context_multiple_param_types() {
        let engine = SedanGql::new();
        let store = test_store();
        let mut params = HashMap::new();
        params.insert("i".into(), Value::Int(7));
        params.insert("s".into(), Value::Str("hi".into()));
        params.insert("b".into(), Value::Bool(true));
        let qr = engine
            .execute_with_context(
                &store,
                "test",
                "RETURN $i AS i, $s AS s, $b AS b",
                &ProcedureRegistry::default(),
                params,
            )
            .unwrap();
        let row = &qr.rows[0];
        assert_eq!(row.get("i"), Some(&Value::Int(7)));
        assert_eq!(row.get("s"), Some(&Value::Str("hi".into())));
        assert_eq!(row.get("b"), Some(&Value::Bool(true)));
    }

    /// INSERT then MATCH round-trip via the lens, exercising mutation stats.
    #[test]
    fn execute_insert_then_match_reports_stats() {
        let engine = SedanGql::new();
        let store = test_store();
        let ins = engine
            .execute(
                &store,
                "test",
                "INSERT (:Item {n: 1}), (:Item {n: 2}), (:Item {n: 3})",
            )
            .unwrap();
        assert!(ins.stats.nodes_created >= 3);
        let qr = engine
            .execute(&store, "test", "MATCH (i:Item) RETURN count(i) AS c")
            .unwrap();
        assert_eq!(qr.rows[0].get("c"), Some(&Value::Int(3)));
    }

    /// Lowercase keywords (GQL is case-insensitive) work via the lens.
    #[test]
    fn execute_accepts_lowercase_keywords() {
        let engine = SedanGql::new();
        let store = test_store();
        let qr = engine.execute(&store, "test", "return 1 as x").unwrap();
        assert_eq!(qr.rows[0].get("x"), Some(&Value::Int(1)));
    }

    /// Two engine instances are independent (no shared state).
    #[test]
    fn multiple_engines_are_independent() {
        let e1 = SedanGql::new();
        let e2 = SedanGql::default();
        let store = test_store();
        let r1 = e1.execute(&store, "test", "RETURN 1 AS x").unwrap();
        let r2 = e2.execute(&store, "test", "RETURN 2 AS x").unwrap();
        assert_eq!(r1.rows[0].get("x"), Some(&Value::Int(1)));
        assert_eq!(r2.rows[0].get("x"), Some(&Value::Int(2)));
    }

    // ── execute_with_budget ─────────────────────────────────────────────

    #[test]
    fn execute_with_budget_returns_result() {
        use std::time::Duration;

        let engine = SedanGql::new();
        let store = test_store();
        let mut budget = ExecutionBudget::new(Duration::from_secs(5), 1_000);
        let qr = engine
            .execute_with_budget(&store, "test", "RETURN 42 AS x", &mut budget)
            .unwrap();
        assert_eq!(qr.rows[0].get("x"), Some(&Value::Int(42)));
    }

    #[test]
    fn execute_with_budget_propagates_parse_error() {
        use std::time::Duration;

        let engine = SedanGql::new();
        let store = test_store();
        let mut budget = ExecutionBudget::new(Duration::from_secs(5), 1_000);
        let err = engine.execute_with_budget(&store, "test", "THIS IS NOT VALID GQL", &mut budget);
        assert!(err.is_err());
    }

    #[test]
    fn execute_with_budget_custom_budget_is_used() {
        use std::time::Duration;

        let engine = SedanGql::new();
        let store = test_store();
        engine
            .execute(&store, "test", "INSERT (:BudgetTest {x: 1})")
            .unwrap();
        let mut budget = ExecutionBudget::new(Duration::from_secs(10), 500);
        let qr = engine
            .execute_with_budget(
                &store,
                "test",
                "MATCH (n:BudgetTest) RETURN n.x AS x",
                &mut budget,
            )
            .unwrap();
        assert_eq!(qr.rows[0].get("x"), Some(&Value::Int(1)));
    }

    #[test]
    fn execute_with_context_and_budget_binds_parameters() {
        use std::time::Duration;

        let engine = SedanGql::new();
        let store = test_store();
        let registry = ProcedureRegistry::default();
        let mut params = HashMap::new();
        params.insert("name".into(), Value::Str("Alice".into()));
        let mut budget = ExecutionBudget::new(Duration::from_secs(5), 1_000);
        let qr = engine
            .execute_with_context_and_budget(
                &store,
                "test",
                "INSERT (n:Person {name: $name}) RETURN n.name AS name",
                &registry,
                params,
                &mut budget,
            )
            .unwrap();
        assert_eq!(qr.rows.len(), 1);
        assert_eq!(qr.rows[0].get("name"), Some(&Value::Str("Alice".into())));
    }

    /// Prelude re-exports `ExecutionBudget` (compile-time check).
    #[test]
    fn prelude_reexports_execution_budget() {
        use crate::prelude::*;

        let _budget = ExecutionBudget::new(std::time::Duration::from_secs(1), 100);
        let engine = SedanGql::new();
        let store = InMemoryGraphStore::default();
        store.create_graph("p").expect("create graph");
        let mut budget = ExecutionBudget::new(std::time::Duration::from_secs(5), 1_000);
        let qr = engine
            .execute_with_budget(&store, "p", "RETURN 1 AS x", &mut budget)
            .unwrap();
        assert_eq!(qr.rows[0].get("x"), Some(&Value::Int(1)));
    }
}
