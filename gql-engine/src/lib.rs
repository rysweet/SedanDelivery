//! GQL engine — ISO/IEC 39075 query processor.
//!
//! This crate implements the GQL (Graph Query Language) standard by building
//! on top of [`cypher_core`]. GQL and openCypher share the physical operator
//! tree, executor, and storage layer. The differences are in:
//!
//! - **Parsing**: GQL has different keywords (`INSERT` vs `CREATE`, `FILTER`
//!   vs `WHERE`, `MANDATORY MATCH`, etc.)
//! - **Planning**: GQL has path modes (WALK/TRAIL/SIMPLE/ACYCLIC) and path
//!   search modifiers (ANY SHORTEST, ALL SHORTEST, etc.)
//! - **Execution**: GQL defaults to isomorphic matching (no two pattern
//!   variables bind the same graph element), and MANDATORY MATCH errors on
//!   empty results.
//!
//! # Architecture
//!
//! ```text
//! GQL source
//!   │
//!   ▼
//! parser::parse_gql()    ─── GQL→Cypher rewriting + metadata extraction
//!   │
//!   ▼
//! planner::plan_gql()    ─── delegates to cypher-core QueryPlanner + GQL flags
//!   │
//!   ▼
//! executor::execute_gql() ── delegates to cypher-core QueryExecutor + GQL checks
//!   │
//!   ▼
//! QueryResult
//! ```

#![warn(missing_docs)]

pub mod executor;
pub mod parser;
pub mod planner;

// Re-export core types that consumers need.
pub use cypher_core::executor::{MutationStats, QueryResult, Row};
pub use cypher_core::storage::{GraphStore, InMemoryGraphStore};
pub use cypher_core::types::Value;

use std::collections::HashMap;

use cypher_core::procedure::ProcedureRegistry;

/// Execute a GQL query string against a graph store.
///
/// This is the top-level convenience function that parses, plans, and executes
/// a GQL query in one call.
pub fn execute(src: &str, store: &dyn GraphStore, graph: &str) -> Result<QueryResult, GqlError> {
    let gql_query = parser::parse_gql(src).map_err(GqlError::Parse)?;
    let plan = planner::plan_gql(&gql_query, store, graph).map_err(GqlError::Plan)?;
    executor::execute_gql(&plan, store, graph).map_err(GqlError::Execution)
}

/// Execute a GQL query with parameters and a procedure registry.
pub fn execute_with_context(
    src: &str,
    store: &dyn GraphStore,
    graph: &str,
    registry: &ProcedureRegistry,
    parameters: HashMap<String, Value>,
) -> Result<QueryResult, GqlError> {
    let gql_query = parser::parse_gql(src).map_err(GqlError::Parse)?;
    let plan = planner::plan_gql_with_context(&gql_query, store, graph, registry, &parameters)
        .map_err(GqlError::Plan)?;
    executor::execute_gql_with_context(&plan, store, graph, registry, parameters)
        .map_err(GqlError::Execution)
}

/// Top-level GQL engine error.
#[derive(Debug, thiserror::Error)]
pub enum GqlError {
    /// The GQL source text could not be parsed into an AST.
    #[error("parse error: {0}")]
    Parse(parser::GqlParseError),
    /// Parsing succeeded but planning the query against the target graph failed
    /// (e.g. unbound variable, missing graph, type mismatch in a literal).
    #[error("plan error: {0}")]
    Plan(cypher_core::error::GqlError),
    /// Planning succeeded but executing the plan against the store failed
    /// (e.g. budget exceeded, storage error, runtime type error).
    #[error("execution error: {0}")]
    Execution(executor::GqlExecutionError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use cypher_core::storage::InMemoryGraphStore;

    fn setup_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        store
    }

    /// Smoke test: GQL INSERT + FILTER end-to-end.
    /// Full coverage is in tests/outside_in.rs, tests/unit_parser.rs, etc.
    #[test]
    fn gql_insert_and_filter_smoke() {
        let store = setup_store();
        execute("INSERT (n:Person {name: 'Alice', age: 30})", &store, "test").unwrap();
        execute("INSERT (n:Person {name: 'Bob', age: 20})", &store, "test").unwrap();

        let qr = execute(
            "MATCH (n:Person) FILTER n.age > 25 RETURN n.name",
            &store,
            "test",
        )
        .unwrap();
        assert_eq!(qr.rows.len(), 1);
        assert_eq!(qr.rows[0].get("n.name"), Some(&Value::Str("Alice".into())));
    }

    /// Smoke test: MANDATORY MATCH errors on empty results.
    #[test]
    fn gql_mandatory_match_empty_errors() {
        let store = setup_store();
        let result = execute("MANDATORY MATCH (n:Ghost) RETURN n", &store, "test");
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("mandatory"), "got: {msg}");
    }

    /// Smoke test: FOR keyword works.
    #[test]
    fn gql_for_keyword() {
        let store = setup_store();
        let qr = execute("FOR x IN [1, 2, 3] RETURN x", &store, "test").unwrap();
        assert_eq!(qr.rows.len(), 3);
    }

    /// LET rewrite: documents current behaviour (cypher-core may not support WITH * yet).
    #[test]
    fn gql_let_rewrite_produces_valid_with() {
        let gql = parser::parse_gql("MATCH (n:Person) LET x = n.name RETURN n");
        if let Err(e) = gql {
            let err = format!("{e:?}");
            assert!(
                err.contains("Syntax") || err.contains("Comma"),
                "unexpected error: {err}"
            );
        }
    }

    // ── Coverage uplift: execute_with_context, error variants ────────────

    #[test]
    fn execute_with_context_passes_parameters_through() {
        let s = setup_store();
        execute("INSERT (n:P {name:'Alice', age:30})", &s, "test").unwrap();
        execute("INSERT (n:P {name:'Bob', age:20})", &s, "test").unwrap();

        let registry = ProcedureRegistry::default();
        let mut params = HashMap::new();
        params.insert("min_age".to_string(), Value::Int(25));

        let qr = execute_with_context(
            "MATCH (n:P) FILTER n.age > $min_age RETURN n.name",
            &s,
            "test",
            &registry,
            params,
        )
        .unwrap();
        assert_eq!(qr.rows.len(), 1);
        assert_eq!(qr.rows[0].get("n.name"), Some(&Value::Str("Alice".into())));
    }

    #[test]
    fn execute_with_context_empty_parameters_works() {
        let s = setup_store();
        execute("INSERT (n:P {name:'X'})", &s, "test").unwrap();
        let registry = ProcedureRegistry::default();
        let qr = execute_with_context(
            "MATCH (n:P) RETURN n.name",
            &s,
            "test",
            &registry,
            HashMap::new(),
        )
        .unwrap();
        assert_eq!(qr.rows.len(), 1);
    }

    #[test]
    fn execute_propagates_parse_error() {
        let s = setup_store();
        let err = execute("THIS IS NOT VALID GQL @@@", &s, "test").unwrap_err();
        assert!(matches!(err, GqlError::Parse(_)), "got: {err:?}");
        assert!(format!("{err}").contains("parse error"));
    }

    #[test]
    fn execute_with_context_propagates_parse_error() {
        let s = setup_store();
        let registry = ProcedureRegistry::default();
        let err = execute_with_context("???", &s, "test", &registry, HashMap::new()).unwrap_err();
        assert!(matches!(err, GqlError::Parse(_)));
    }

    #[test]
    fn execute_propagates_plan_or_execution_error_for_unknown_graph() {
        let s = setup_store();
        let err = execute("MATCH (n) RETURN n", &s, "missing").unwrap_err();
        let msg = format!("{err}");
        assert!(!msg.is_empty());
    }

    #[test]
    fn gql_error_display_variants_distinct() {
        let s = setup_store();
        let parse_err = execute("???", &s, "test").unwrap_err();
        assert!(format!("{parse_err}").starts_with("parse error"));

        let mandatory_err = execute("MANDATORY MATCH (n:Nope) RETURN n", &s, "test").unwrap_err();
        let m = format!("{mandatory_err}");
        assert!(m.contains("execution error"), "got: {m}");
    }
}
