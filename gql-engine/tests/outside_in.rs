//! Outside-in integration tests for gql-engine.
//!
//! These tests exercise the public API as an external consumer would,
//! validating that GQL-specific syntax produces correct results when
//! run end-to-end through the engine.

use gql_engine::{execute, GraphStore, InMemoryGraphStore, QueryResult, Value};

fn fresh_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

fn assert_ok(result: Result<QueryResult, gql_engine::GqlError>) -> QueryResult {
    result.expect("query should succeed")
}

// ── Scenario 1: GQL INSERT keyword (differs from openCypher CREATE) ──

#[test]
fn insert_creates_node_and_returns_property() {
    let store = fresh_store();
    let qr = assert_ok(execute(
        "INSERT (n:Person {name: 'Alice'}) RETURN n.name",
        &store,
        "g",
    ));
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("n.name"), Some(&Value::Str("Alice".into())));
}

#[test]
fn insert_creates_relationship() {
    let store = fresh_store();
    let qr = assert_ok(execute(
        "INSERT (a:Person {name: 'A'})-[:KNOWS]->(b:Person {name: 'B'}) RETURN a.name, b.name",
        &store,
        "g",
    ));
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("a.name"), Some(&Value::Str("A".into())));
    assert_eq!(qr.rows[0].get("b.name"), Some(&Value::Str("B".into())));
}

// ── Scenario 2: GQL FILTER clause (differs from openCypher WHERE) ──

#[test]
fn filter_clause_selects_matching_nodes() {
    let store = fresh_store();
    assert_ok(execute(
        "CREATE (n:Person {name: 'Alice', age: 30})",
        &store,
        "g",
    ));
    assert_ok(execute(
        "CREATE (n:Person {name: 'Bob', age: 20})",
        &store,
        "g",
    ));

    let qr = assert_ok(execute(
        "MATCH (n:Person) FILTER n.age > 25 RETURN n.name",
        &store,
        "g",
    ));
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("n.name"), Some(&Value::Str("Alice".into())));
}

#[test]
fn filter_where_variant_works() {
    let store = fresh_store();
    assert_ok(execute(
        "CREATE (n:Person {name: 'Alice', age: 30})",
        &store,
        "g",
    ));

    let qr = assert_ok(execute(
        "MATCH (n:Person) FILTER WHERE n.age = 30 RETURN n.name",
        &store,
        "g",
    ));
    assert_eq!(qr.rows.len(), 1);
}

// ── Scenario 3: GQL MANDATORY MATCH ──

#[test]
fn mandatory_match_errors_on_empty_result() {
    let store = fresh_store();
    let result = execute("MANDATORY MATCH (n:Ghost) RETURN n", &store, "g");
    assert!(result.is_err(), "MANDATORY MATCH with no data should error");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.to_lowercase().contains("mandatory"),
        "error should mention mandatory, got: {msg}"
    );
}

#[test]
fn mandatory_match_succeeds_with_data() {
    let store = fresh_store();
    assert_ok(execute("CREATE (n:Person {name: 'Alice'})", &store, "g"));

    let qr = assert_ok(execute(
        "MANDATORY MATCH (n:Person) RETURN n.name",
        &store,
        "g",
    ));
    assert_eq!(qr.rows.len(), 1);
}

// ── Scenario 4: GQL FOR keyword (differs from openCypher UNWIND) ──

#[test]
fn for_keyword_unwinds_list() {
    let store = fresh_store();
    let qr = assert_ok(execute("FOR x IN [1, 2, 3] RETURN x", &store, "g"));
    assert_eq!(qr.rows.len(), 3);
}

// ── Scenario 5: Path mode metadata extraction ──

#[test]
fn path_modes_are_parsed() {
    use gql_engine::parser::{parse_gql, PathMode};

    for (keyword, expected) in [
        ("WALK", PathMode::Walk),
        ("TRAIL", PathMode::Trail),
        ("SIMPLE", PathMode::Simple),
        ("ACYCLIC", PathMode::Acyclic),
    ] {
        let src = format!("MATCH {keyword} (a)-[:R]->(b) RETURN a");
        let gql = parse_gql(&src).unwrap_or_else(|e| panic!("{keyword} failed: {e:?}"));
        assert_eq!(
            gql.metadata.path_mode,
            Some(expected),
            "path mode mismatch for {keyword}"
        );
    }
}

#[test]
fn path_search_any_shortest_parsed() {
    use gql_engine::parser::{parse_gql, PathSearch};

    let gql =
        parse_gql("MATCH ANY SHORTEST (a)-[:R]->(b) RETURN a").expect("ANY SHORTEST should parse");
    assert_eq!(gql.metadata.path_search, Some(PathSearch::AnyShortest));
}

// ── Scenario 6: Standard Cypher still works through GQL engine ──

#[test]
fn standard_cypher_create_match_works() {
    let store = fresh_store();
    assert_ok(execute(
        "CREATE (a:Person {name: 'Alice'})-[:KNOWS]->(b:Person {name: 'Bob'})",
        &store,
        "g",
    ));

    let qr = assert_ok(execute(
        "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name",
        &store,
        "g",
    ));
    assert_eq!(qr.rows.len(), 1);
}

// ── Scenario 7: Mixed GQL keywords in one workflow ──

#[test]
fn mixed_gql_workflow_insert_then_filter() {
    let store = fresh_store();
    // Use GQL INSERT to create data
    assert_ok(execute(
        "INSERT (n:City {name: 'London', pop: 9000000})",
        &store,
        "g",
    ));
    assert_ok(execute(
        "INSERT (n:City {name: 'Bath', pop: 90000})",
        &store,
        "g",
    ));

    // Use GQL FILTER to query
    let qr = assert_ok(execute(
        "MATCH (n:City) FILTER n.pop > 1000000 RETURN n.name",
        &store,
        "g",
    ));
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("n.name"), Some(&Value::Str("London".into())));
}
