//! Parity tests: GQL engine produces same results as cypher-core for shared syntax.
//!
//! These tests verify that queries using standard Cypher syntax (not GQL-only
//! keywords) produce identical results whether run through gql_engine::execute
//! or directly through cypher-core. This guards against regressions in the
//! GQL→Cypher rewrite layer corrupting shared syntax.

use gql_engine::{execute as gql_execute, GraphStore, InMemoryGraphStore};

fn fresh_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

/// Execute a Cypher query directly through cypher-core (no GQL rewriting).
fn cypher_execute(
    src: &str,
    store: &dyn GraphStore,
    graph: &str,
) -> Result<cypher_core::executor::QueryResult, cypher_core::error::GqlError> {
    let query = cypher_core::parser::parse_query(src)?;
    let planner = cypher_core::planner::QueryPlanner::new(store, graph);
    let operator = planner.plan(&query)?;
    let executor = cypher_core::executor::QueryExecutor::new(store);
    let mut budget = cypher_core::executor::ExecutionBudget::default_budget();
    executor.execute(graph, &operator, &mut budget)
}

// ── RETURN literal parity ────────────────────────────────────────────────────

#[test]
fn return_integer_literal_parity() {
    let store = fresh_store();
    let gql = gql_execute("RETURN 42 AS val", &store, "g").unwrap();
    let cyp = cypher_execute("RETURN 42 AS val", &store, "g").unwrap();
    assert_eq!(gql.rows, cyp.rows);
    assert_eq!(gql.columns, cyp.columns);
}

#[test]
fn return_string_literal_parity() {
    let store = fresh_store();
    let gql = gql_execute("RETURN 'hello' AS val", &store, "g").unwrap();
    let cyp = cypher_execute("RETURN 'hello' AS val", &store, "g").unwrap();
    assert_eq!(gql.rows, cyp.rows);
}

#[test]
fn return_boolean_parity() {
    let store = fresh_store();
    let gql = gql_execute("RETURN true AS val", &store, "g").unwrap();
    let cyp = cypher_execute("RETURN true AS val", &store, "g").unwrap();
    assert_eq!(gql.rows, cyp.rows);
}

#[test]
fn return_null_parity() {
    let store = fresh_store();
    let gql = gql_execute("RETURN null AS val", &store, "g").unwrap();
    let cyp = cypher_execute("RETURN null AS val", &store, "g").unwrap();
    assert_eq!(gql.rows, cyp.rows);
}

#[test]
fn return_list_parity() {
    let store = fresh_store();
    let gql = gql_execute("RETURN [1, 2, 3] AS val", &store, "g").unwrap();
    let cyp = cypher_execute("RETURN [1, 2, 3] AS val", &store, "g").unwrap();
    assert_eq!(gql.rows, cyp.rows);
}

// ── Arithmetic parity ────────────────────────────────────────────────────────

#[test]
fn arithmetic_expression_parity() {
    let store = fresh_store();
    let gql = gql_execute("RETURN 2 + 3 * 4 AS val", &store, "g").unwrap();
    let cyp = cypher_execute("RETURN 2 + 3 * 4 AS val", &store, "g").unwrap();
    assert_eq!(gql.rows, cyp.rows);
}

// ── CREATE + MATCH parity ────────────────────────────────────────────────────

#[test]
fn create_and_match_parity() {
    // Use two separate stores to ensure independent state
    let gql_store = fresh_store();
    let cyp_store = fresh_store();

    gql_execute(
        "CREATE (n:Person {name: 'Alice', age: 30})",
        &gql_store,
        "g",
    )
    .unwrap();
    cypher_execute(
        "CREATE (n:Person {name: 'Alice', age: 30})",
        &cyp_store,
        "g",
    )
    .unwrap();

    let gql = gql_execute("MATCH (n:Person) RETURN n.name, n.age", &gql_store, "g").unwrap();
    let cyp = cypher_execute("MATCH (n:Person) RETURN n.name, n.age", &cyp_store, "g").unwrap();

    assert_eq!(gql.rows.len(), cyp.rows.len());
    assert_eq!(gql.columns, cyp.columns);
    // Compare values (order may differ, so check set equality)
    assert_eq!(gql.rows[0].get("n.name"), cyp.rows[0].get("n.name"));
    assert_eq!(gql.rows[0].get("n.age"), cyp.rows[0].get("n.age"));
}

// ── WHERE clause parity ──────────────────────────────────────────────────────

#[test]
fn where_clause_parity() {
    let gql_store = fresh_store();
    let cyp_store = fresh_store();

    for store in [&gql_store, &cyp_store] {
        cypher_execute("CREATE (n:Person {name: 'Alice', age: 30})", store, "g").unwrap();
        cypher_execute("CREATE (n:Person {name: 'Bob', age: 20})", store, "g").unwrap();
    }

    // GQL WHERE is the same as Cypher WHERE (FILTER is the GQL-specific variant)
    let query = "MATCH (n:Person) WHERE n.age > 25 RETURN n.name";
    let gql = gql_execute(query, &gql_store, "g").unwrap();
    let cyp = cypher_execute(query, &cyp_store, "g").unwrap();

    assert_eq!(gql.rows.len(), cyp.rows.len());
    assert_eq!(gql.rows[0].get("n.name"), cyp.rows[0].get("n.name"));
}

// ── Relationship parity ─────────────────────────────────────────────────────

#[test]
fn relationship_create_and_match_parity() {
    let gql_store = fresh_store();
    let cyp_store = fresh_store();

    let create = "CREATE (a:Person {name: 'Alice'})-[:KNOWS]->(b:Person {name: 'Bob'})";
    gql_execute(create, &gql_store, "g").unwrap();
    cypher_execute(create, &cyp_store, "g").unwrap();

    let query = "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name";
    let gql = gql_execute(query, &gql_store, "g").unwrap();
    let cyp = cypher_execute(query, &cyp_store, "g").unwrap();

    assert_eq!(gql.rows.len(), cyp.rows.len());
    assert_eq!(gql.rows[0].get("a.name"), cyp.rows[0].get("a.name"));
    assert_eq!(gql.rows[0].get("b.name"), cyp.rows[0].get("b.name"));
}

// ── UNWIND parity ────────────────────────────────────────────────────────────

#[test]
fn unwind_parity() {
    let store = fresh_store();
    let gql = gql_execute("UNWIND [1, 2, 3] AS x RETURN x", &store, "g").unwrap();
    let cyp = cypher_execute("UNWIND [1, 2, 3] AS x RETURN x", &store, "g").unwrap();
    assert_eq!(gql.rows, cyp.rows);
}

// ── Multiple RETURN columns parity ───────────────────────────────────────────

#[test]
fn multiple_return_columns_parity() {
    let store = fresh_store();
    let gql = gql_execute("RETURN 1 AS a, 'two' AS b, true AS c", &store, "g").unwrap();
    let cyp = cypher_execute("RETURN 1 AS a, 'two' AS b, true AS c", &store, "g").unwrap();
    assert_eq!(gql.rows, cyp.rows);
    assert_eq!(gql.columns, cyp.columns);
}

// ── Error parity ─────────────────────────────────────────────────────────────

#[test]
fn invalid_syntax_errors_in_both() {
    let store = fresh_store();
    let gql = gql_execute("XYZZY NONSENSE", &store, "g");
    let cyp = cypher_execute("XYZZY NONSENSE", &store, "g");
    assert!(gql.is_err());
    assert!(cyp.is_err());
}

// ── GQL-specific: INSERT produces same result as CREATE ──────────────────────

#[test]
fn gql_insert_matches_cypher_create() {
    let gql_store = fresh_store();
    let cyp_store = fresh_store();

    let gql = gql_execute(
        "INSERT (n:Person {name: 'Alice'}) RETURN n.name",
        &gql_store,
        "g",
    )
    .unwrap();
    let cyp = cypher_execute(
        "CREATE (n:Person {name: 'Alice'}) RETURN n.name",
        &cyp_store,
        "g",
    )
    .unwrap();

    assert_eq!(gql.rows.len(), cyp.rows.len());
    assert_eq!(gql.rows[0].get("n.name"), cyp.rows[0].get("n.name"));
}

// ── GQL FILTER produces same result as Cypher WHERE ──────────────────────────

#[test]
fn gql_filter_matches_cypher_where() {
    let gql_store = fresh_store();
    let cyp_store = fresh_store();

    for store in [&gql_store, &cyp_store] {
        cypher_execute("CREATE (n:Person {name: 'Alice', age: 30})", store, "g").unwrap();
        cypher_execute("CREATE (n:Person {name: 'Bob', age: 20})", store, "g").unwrap();
    }

    let gql = gql_execute(
        "MATCH (n:Person) FILTER n.age > 25 RETURN n.name",
        &gql_store,
        "g",
    )
    .unwrap();
    let cyp = cypher_execute(
        "MATCH (n:Person) WHERE n.age > 25 RETURN n.name",
        &cyp_store,
        "g",
    )
    .unwrap();

    assert_eq!(gql.rows.len(), cyp.rows.len());
    assert_eq!(gql.rows[0].get("n.name"), cyp.rows[0].get("n.name"));
}

// ── GQL FOR produces same result as Cypher UNWIND ────────────────────────────

#[test]
fn gql_for_matches_cypher_unwind() {
    let store = fresh_store();
    let gql = gql_execute("FOR x IN [10, 20, 30] RETURN x", &store, "g").unwrap();
    let cyp = cypher_execute("UNWIND [10, 20, 30] AS x RETURN x", &store, "g").unwrap();
    assert_eq!(gql.rows, cyp.rows);
}

// ── LIMIT / ORDER BY parity ─────────────────────────────────────────────────

#[test]
fn order_by_parity() {
    let gql_store = fresh_store();
    let cyp_store = fresh_store();

    for store in [&gql_store, &cyp_store] {
        cypher_execute("CREATE (n:N {val: 3})", store, "g").unwrap();
        cypher_execute("CREATE (n:N {val: 1})", store, "g").unwrap();
        cypher_execute("CREATE (n:N {val: 2})", store, "g").unwrap();
    }

    let query = "MATCH (n:N) RETURN n.val ORDER BY n.val";
    let gql = gql_execute(query, &gql_store, "g").unwrap();
    let cyp = cypher_execute(query, &cyp_store, "g").unwrap();
    assert_eq!(gql.rows, cyp.rows);
}

#[test]
fn limit_parity() {
    let gql_store = fresh_store();
    let cyp_store = fresh_store();

    for store in [&gql_store, &cyp_store] {
        for i in 0..5 {
            cypher_execute(&format!("CREATE (n:N {{val: {i}}})"), store, "g").unwrap();
        }
    }

    let query = "MATCH (n:N) RETURN n.val ORDER BY n.val LIMIT 3";
    let gql = gql_execute(query, &gql_store, "g").unwrap();
    let cyp = cypher_execute(query, &cyp_store, "g").unwrap();
    assert_eq!(gql.rows.len(), 3);
    assert_eq!(gql.rows, cyp.rows);
}

// ── NULL expression parity ──────────────────────────────────────────────────

#[test]
fn null_arithmetic_parity() {
    let store = fresh_store();
    let gql = gql_execute("RETURN null + 1 AS val", &store, "g").unwrap();
    let cyp = cypher_execute("RETURN null + 1 AS val", &store, "g").unwrap();
    assert_eq!(gql.rows, cyp.rows);
}

// ── DELETE parity ───────────────────────────────────────────────────────────

#[test]
fn delete_parity() {
    let gql_store = fresh_store();
    let cyp_store = fresh_store();

    for store in [&gql_store, &cyp_store] {
        cypher_execute("CREATE (n:N {val: 1})", store, "g").unwrap();
        cypher_execute("CREATE (n:N {val: 2})", store, "g").unwrap();
    }

    let delete_query = "MATCH (n:N {val: 1}) DELETE n";
    gql_execute(delete_query, &gql_store, "g").unwrap();
    cypher_execute(delete_query, &cyp_store, "g").unwrap();

    let count_query = "MATCH (n:N) RETURN n.val";
    let gql = gql_execute(count_query, &gql_store, "g").unwrap();
    let cyp = cypher_execute(count_query, &cyp_store, "g").unwrap();
    assert_eq!(gql.rows.len(), cyp.rows.len());
    assert_eq!(gql.rows.len(), 1);
}

// ── GQL INSERT relationship parity with Cypher CREATE relationship ──────────

#[test]
fn gql_insert_relationship_matches_cypher_create_relationship() {
    let gql_store = fresh_store();
    let cyp_store = fresh_store();

    gql_execute(
        "INSERT (a:Person {name: 'A'})-[:KNOWS]->(b:Person {name: 'B'})",
        &gql_store,
        "g",
    )
    .unwrap();
    cypher_execute(
        "CREATE (a:Person {name: 'A'})-[:KNOWS]->(b:Person {name: 'B'})",
        &cyp_store,
        "g",
    )
    .unwrap();

    let query = "MATCH (a)-[:KNOWS]->(b) RETURN a.name, b.name";
    let gql = gql_execute(query, &gql_store, "g").unwrap();
    let cyp = cypher_execute(query, &cyp_store, "g").unwrap();

    assert_eq!(gql.rows.len(), cyp.rows.len());
    assert_eq!(gql.rows[0].get("a.name"), cyp.rows[0].get("a.name"));
}
