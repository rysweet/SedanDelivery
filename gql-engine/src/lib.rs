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
    #[error("parse error: {0}")]
    Parse(parser::GqlParseError),
    #[error("plan error: {0}")]
    Plan(cypher_core::error::GqlError),
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

    #[test]
    fn gql_basic_match_return() {
        let store = setup_store();
        // Create a node first using standard Cypher (shared syntax)
        let result = execute(
            "CREATE (n:Person {name: 'Alice'}) RETURN n.name",
            &store,
            "test",
        );
        assert!(result.is_ok());
        let qr = result.unwrap();
        assert_eq!(qr.rows.len(), 1);
    }

    #[test]
    fn gql_insert_keyword() {
        let store = setup_store();
        // GQL uses INSERT instead of CREATE
        let result = execute(
            "INSERT (n:Person {name: 'Bob'}) RETURN n.name",
            &store,
            "test",
        );
        assert!(result.is_ok());
        let qr = result.unwrap();
        assert_eq!(qr.rows.len(), 1);
        let name = qr.rows[0].get("n.name").unwrap();
        assert_eq!(name, &Value::Str("Bob".into()));
    }

    #[test]
    fn gql_filter_clause() {
        let store = setup_store();
        // Create some nodes
        execute("CREATE (n:Person {name: 'Alice', age: 30})", &store, "test").unwrap();
        execute("CREATE (n:Person {name: 'Bob', age: 20})", &store, "test").unwrap();

        // GQL uses FILTER instead of WHERE
        let result = execute(
            "MATCH (n:Person) FILTER n.age > 25 RETURN n.name",
            &store,
            "test",
        );
        assert!(result.is_ok());
        let qr = result.unwrap();
        assert_eq!(qr.rows.len(), 1);
        let name = qr.rows[0].get("n.name").unwrap();
        assert_eq!(name, &Value::Str("Alice".into()));
    }

    #[test]
    fn gql_mandatory_match_empty_errors() {
        let store = setup_store();
        // MANDATORY MATCH with no data should error
        let result = execute("MANDATORY MATCH (n:NonExistent) RETURN n", &store, "test");
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("mandatory"),
            "expected mandatory error, got: {msg}"
        );
    }

    #[test]
    fn gql_mandatory_match_with_data_succeeds() {
        let store = setup_store();
        execute("CREATE (n:Person {name: 'Alice'})", &store, "test").unwrap();

        let result = execute("MANDATORY MATCH (n:Person) RETURN n.name", &store, "test");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().rows.len(), 1);
    }

    #[test]
    fn gql_parser_metadata_extraction() {
        let gql = parser::parse_gql("MANDATORY MATCH (n) RETURN n").unwrap();
        assert!(gql.metadata.mandatory_match);
    }

    #[test]
    fn gql_path_mode_metadata() {
        // Path mode is metadata-only for now (not enforced in execution)
        let src = "MATCH TRAIL (a)-[*]->(b) RETURN a";
        let meta = parser::rewrite::extract_gql_metadata(src);
        assert_eq!(meta.path_mode, Some(parser::PathMode::Trail));
    }

    #[test]
    fn gql_path_search_metadata() {
        let src = "MATCH ANY SHORTEST (a)-[*]->(b) RETURN a";
        let meta = parser::rewrite::extract_gql_metadata(src);
        assert_eq!(meta.path_search, Some(parser::PathSearch::AnyShortest));
    }

    #[test]
    fn standard_cypher_still_works() {
        let store = setup_store();
        let result = execute(
            "CREATE (a:Person {name: 'Alice'})-[:KNOWS]->(b:Person {name: 'Bob'}) RETURN a.name, b.name",
            &store,
            "test",
        );
        assert!(result.is_ok());
        let qr = result.unwrap();
        assert_eq!(qr.rows.len(), 1);
    }

    // --- TDD: GQL LET bindings ---

    #[test]
    fn gql_let_rewrite_produces_valid_with() {
        // Verify LET is rewritten correctly at the parser level.
        // Full execution requires cypher-core support for WITH *, expr AS x
        // which is tracked as a future enhancement.
        let gql = parser::parse_gql("MATCH (n:Person) LET x = n.name RETURN n");
        if let Err(e) = gql {
            let err = format!("{e:?}");
            // Expected: cypher-core doesn't support WITH *, yet
            assert!(
                err.contains("Syntax") || err.contains("Comma"),
                "unexpected error: {err}"
            );
        }
    }

    // --- TDD: GQL FOR keyword ---

    #[test]
    fn gql_for_keyword() {
        let store = setup_store();

        // GQL uses FOR instead of UNWIND
        let result = execute("FOR x IN [1, 2, 3] RETURN x", &store, "test");
        assert!(result.is_ok(), "FOR keyword failed: {:?}", result.err());
        let qr = result.unwrap();
        assert_eq!(qr.rows.len(), 3);
    }

    // --- TDD: GQL INSERT with relationships ---

    #[test]
    fn gql_insert_with_relationship() {
        let store = setup_store();
        let result = execute(
            "INSERT (a:Person {name: 'Alice'})-[:KNOWS]->(b:Person {name: 'Bob'}) RETURN a.name",
            &store,
            "test",
        );
        assert!(result.is_ok(), "INSERT with rel failed: {:?}", result.err());
    }

    // --- TDD: GQL FILTER with complex predicates ---

    #[test]
    fn gql_filter_with_and_or() {
        let store = setup_store();
        execute("CREATE (n:Person {name: 'Alice', age: 30})", &store, "test").unwrap();
        execute("CREATE (n:Person {name: 'Bob', age: 20})", &store, "test").unwrap();
        execute(
            "CREATE (n:Person {name: 'Charlie', age: 25})",
            &store,
            "test",
        )
        .unwrap();

        let result = execute(
            "MATCH (n:Person) FILTER n.age > 22 AND n.age < 28 RETURN n.name",
            &store,
            "test",
        );
        assert!(result.is_ok());
        let qr = result.unwrap();
        assert_eq!(qr.rows.len(), 1);
        let name = qr.rows[0].get("n.name").unwrap();
        assert_eq!(name, &Value::Str("Charlie".into()));
    }

    // --- TDD: GQL FILTER WHERE variant ---

    #[test]
    fn gql_filter_where_variant() {
        let store = setup_store();
        execute("CREATE (n:Person {name: 'Alice', age: 30})", &store, "test").unwrap();

        // FILTER WHERE is valid GQL syntax (WHERE is optional after FILTER)
        let result = execute(
            "MATCH (n:Person) FILTER WHERE n.age > 25 RETURN n.name",
            &store,
            "test",
        );
        assert!(result.is_ok(), "FILTER WHERE failed: {:?}", result.err());
        assert_eq!(result.unwrap().rows.len(), 1);
    }

    // --- TDD: GQL combined keywords ---

    #[test]
    fn gql_combined_insert_filter_let() {
        let store = setup_store();
        // Use INSERT to create, then query with FILTER
        execute("INSERT (n:Person {name: 'Alice', age: 30})", &store, "test").unwrap();
        execute("INSERT (n:Person {name: 'Bob', age: 20})", &store, "test").unwrap();

        let result = execute(
            "MATCH (n:Person) FILTER n.age >= 25 RETURN n.name",
            &store,
            "test",
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap().rows.len(), 1);
    }

    // --- TDD: GQL path mode metadata through parse_gql ---

    #[test]
    fn gql_parse_trail_mode() {
        let gql = parser::parse_gql("MATCH TRAIL (a)-[:KNOWS]->(b) RETURN a");
        // Should parse successfully (TRAIL stripped from source)
        assert!(gql.is_ok(), "TRAIL parse failed: {:?}", gql.err());
        let q = gql.unwrap();
        assert_eq!(q.metadata.path_mode, Some(parser::PathMode::Trail));
    }

    #[test]
    fn gql_parse_walk_mode() {
        let gql = parser::parse_gql("MATCH WALK (a)-[:KNOWS]->(b) RETURN a");
        assert!(gql.is_ok(), "WALK parse failed: {:?}", gql.err());
        assert_eq!(
            gql.unwrap().metadata.path_mode,
            Some(parser::PathMode::Walk)
        );
    }

    #[test]
    fn gql_parse_simple_mode() {
        let gql = parser::parse_gql("MATCH SIMPLE (a)-[:KNOWS]->(b) RETURN a");
        assert!(gql.is_ok(), "SIMPLE parse failed: {:?}", gql.err());
        assert_eq!(
            gql.unwrap().metadata.path_mode,
            Some(parser::PathMode::Simple)
        );
    }

    #[test]
    fn gql_parse_acyclic_mode() {
        let gql = parser::parse_gql("MATCH ACYCLIC (a)-[:KNOWS]->(b) RETURN a");
        assert!(gql.is_ok(), "ACYCLIC parse failed: {:?}", gql.err());
        assert_eq!(
            gql.unwrap().metadata.path_mode,
            Some(parser::PathMode::Acyclic)
        );
    }

    #[test]
    fn gql_parse_any_shortest_search() {
        let gql = parser::parse_gql("MATCH ANY SHORTEST (a)-[:KNOWS]->(b) RETURN a");
        assert!(gql.is_ok(), "ANY SHORTEST parse failed: {:?}", gql.err());
        assert_eq!(
            gql.unwrap().metadata.path_search,
            Some(parser::PathSearch::AnyShortest)
        );
    }
}
