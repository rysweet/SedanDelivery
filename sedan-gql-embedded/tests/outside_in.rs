//! Outside-in integration tests for sedan-gql-embedded.
//!
//! Exercises the public SedanGql API as an external consumer would,
//! validating GQL-specific syntax, error contracts, and store isolation.

use sedan_gql_embedded::{GqlError, GraphStore, InMemoryGraphStore, QueryResult, SedanGql, Value};

fn engine() -> SedanGql {
    SedanGql::new()
}

fn fresh_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").expect("create graph");
    store
}

fn assert_ok(result: Result<QueryResult, GqlError>) -> QueryResult {
    result.expect("query should succeed")
}

// ── Scenario 1: Basic CRUD lifecycle ─────────────────────────────────────────

#[test]
fn create_match_update_delete_lifecycle() {
    let e = engine();
    let store = fresh_store();

    // Create
    assert_ok(e.execute(&store, "g", "CREATE (n:Person {name: 'Alice', age: 30})"));

    // Read
    let result = assert_ok(e.execute(&store, "g", "MATCH (n:Person) RETURN n.name, n.age"));
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].get("n.name"),
        Some(&Value::Str("Alice".into()))
    );

    // Update
    assert_ok(e.execute(
        &store,
        "g",
        "MATCH (n:Person {name: 'Alice'}) SET n.age = 31 RETURN n.age",
    ));
    let updated =
        assert_ok(e.execute(&store, "g", "MATCH (n:Person {name: 'Alice'}) RETURN n.age"));
    assert_eq!(updated.rows[0].get("n.age"), Some(&Value::Int(31)));

    // Delete
    assert_ok(e.execute(&store, "g", "MATCH (n:Person {name: 'Alice'}) DELETE n"));
    let gone = assert_ok(e.execute(&store, "g", "MATCH (n:Person) RETURN n"));
    assert_eq!(gone.rows.len(), 0);
}

// ── Scenario 2: GQL INSERT creates nodes and relationships ───────────────────

#[test]
fn gql_insert_node_with_properties() {
    let e = engine();
    let store = fresh_store();
    let qr = assert_ok(e.execute(
        &store,
        "g",
        "INSERT (n:Movie {title: 'Arrival', year: 2016}) RETURN n.title, n.year",
    ));
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(
        qr.rows[0].get("n.title"),
        Some(&Value::Str("Arrival".into()))
    );
    assert_eq!(qr.rows[0].get("n.year"), Some(&Value::Int(2016)));
}

#[test]
fn gql_insert_relationship() {
    let e = engine();
    let store = fresh_store();
    assert_ok(e.execute(
        &store,
        "g",
        "INSERT (a:Person {name: 'Alice'})-[:FRIENDS_WITH]->(b:Person {name: 'Bob'})",
    ));

    let qr = assert_ok(e.execute(
        &store,
        "g",
        "MATCH (a:Person)-[:FRIENDS_WITH]->(b:Person) RETURN a.name, b.name",
    ));
    assert_eq!(qr.rows.len(), 1);
}

// ── Scenario 3: GQL FILTER clause ───────────────────────────────────────────

#[test]
fn gql_filter_with_comparison() {
    let e = engine();
    let store = fresh_store();
    assert_ok(e.execute(&store, "g", "CREATE (n:N {val: 10})"));
    assert_ok(e.execute(&store, "g", "CREATE (n:N {val: 20})"));
    assert_ok(e.execute(&store, "g", "CREATE (n:N {val: 30})"));

    let qr = assert_ok(e.execute(&store, "g", "MATCH (n:N) FILTER n.val > 15 RETURN n.val"));
    assert_eq!(qr.rows.len(), 2);
}

#[test]
fn gql_filter_with_and() {
    let e = engine();
    let store = fresh_store();
    assert_ok(e.execute(&store, "g", "CREATE (n:N {val: 10, name: 'a'})"));
    assert_ok(e.execute(&store, "g", "CREATE (n:N {val: 20, name: 'b'})"));
    assert_ok(e.execute(&store, "g", "CREATE (n:N {val: 30, name: 'c'})"));

    let qr = assert_ok(e.execute(
        &store,
        "g",
        "MATCH (n:N) FILTER n.val > 15 AND n.val < 25 RETURN n.name",
    ));
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("n.name"), Some(&Value::Str("b".into())));
}

// ── Scenario 4: GQL MANDATORY MATCH ─────────────────────────────────────────

#[test]
fn mandatory_match_errors_when_empty() {
    let e = engine();
    let store = fresh_store();
    let err = e
        .execute(&store, "g", "MANDATORY MATCH (n:Ghost) RETURN n")
        .unwrap_err();
    let msg = err.to_string().to_lowercase();
    assert!(msg.contains("mandatory"), "got: {msg}");
}

#[test]
fn mandatory_match_succeeds_with_data() {
    let e = engine();
    let store = fresh_store();
    assert_ok(e.execute(&store, "g", "CREATE (n:Person {name: 'Alice'})"));

    let qr = assert_ok(e.execute(&store, "g", "MANDATORY MATCH (n:Person) RETURN n.name"));
    assert_eq!(qr.rows.len(), 1);
}

// ── Scenario 5: GQL FOR keyword ─────────────────────────────────────────────

#[test]
fn for_unwinds_integer_list() {
    let e = engine();
    let store = fresh_store();
    let qr = assert_ok(e.execute(&store, "g", "FOR x IN [1, 2, 3] RETURN x"));
    assert_eq!(qr.rows.len(), 3);
}

#[test]
fn for_unwinds_string_list() {
    let e = engine();
    let store = fresh_store();
    let qr = assert_ok(e.execute(&store, "g", "FOR x IN ['a', 'b'] RETURN x"));
    assert_eq!(qr.rows.len(), 2);
}

// ── Scenario 6: Error handling ──────────────────────────────────────────────

#[test]
fn invalid_query_returns_parse_error() {
    let e = engine();
    let store = fresh_store();
    assert!(e.execute(&store, "g", "NOT VALID !!!").is_err());
}

#[test]
fn missing_graph_returns_error() {
    let e = engine();
    let store = fresh_store();
    let result = e.execute(&store, "nonexistent", "RETURN 1");
    assert!(result.is_err());
}

// ── Scenario 7: Multiple graphs isolation ────────────────────────────────────

#[test]
fn named_graphs_are_isolated() {
    let e = engine();
    let store = InMemoryGraphStore::default();
    store.create_graph("alpha").unwrap();
    store.create_graph("beta").unwrap();

    assert_ok(e.execute(&store, "alpha", "INSERT (n:Person {name: 'Alice'})"));

    let alpha_result = assert_ok(e.execute(&store, "alpha", "MATCH (n:Person) RETURN n.name"));
    assert_eq!(alpha_result.rows.len(), 1);

    let beta_result = assert_ok(e.execute(&store, "beta", "MATCH (n:Person) RETURN n.name"));
    assert_eq!(beta_result.rows.len(), 0);
}

// ── Scenario 8: Store instance isolation ─────────────────────────────────────

#[test]
fn separate_stores_are_isolated() {
    let e = engine();
    let store_a = fresh_store();
    let store_b = fresh_store();

    assert_ok(e.execute(&store_a, "g", "INSERT (n:Person {name: 'Alice'})"));

    let a_result = assert_ok(e.execute(&store_a, "g", "MATCH (n:Person) RETURN n"));
    assert_eq!(a_result.rows.len(), 1);

    let b_result = assert_ok(e.execute(&store_b, "g", "MATCH (n:Person) RETURN n"));
    assert_eq!(b_result.rows.len(), 0);
}

// ── Scenario 9: Column names in QueryResult ──────────────────────────────────

#[test]
fn query_result_has_correct_columns() {
    let e = engine();
    let store = fresh_store();
    let qr = assert_ok(e.execute(&store, "g", "RETURN 1 AS a, 'two' AS b, true AS c"));
    assert_eq!(qr.columns, vec!["a", "b", "c"]);
}

// ── Scenario 10: Default trait impl ──────────────────────────────────────────

#[test]
fn sedan_gql_default_works() {
    let e = SedanGql::default();
    let store = fresh_store();
    let qr = assert_ok(e.execute(&store, "g", "RETURN 42 AS val"));
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Int(42)));
}
