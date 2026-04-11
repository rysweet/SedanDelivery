use sedan_gql_embedded::{GraphStore, InMemoryGraphStore, QueryResult, SedanGql, Value};

fn fresh_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").expect("create graph");
    store
}

fn assert_ok(result: Result<QueryResult, sedan_gql_embedded::GqlError>) -> QueryResult {
    result.expect("query should succeed")
}

#[test]
fn supports_gql_insert_keyword() {
    let engine = SedanGql::new();
    let store = fresh_store();

    let result = assert_ok(engine.execute(
        &store,
        "g",
        "INSERT (n:Person {name: 'Alice'}) RETURN n.name",
    ));

    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].get("n.name"),
        Some(&Value::Str("Alice".into()))
    );
}

#[test]
fn supports_gql_filter_clause() {
    let engine = SedanGql::new();
    let store = fresh_store();

    assert_ok(engine.execute(&store, "g", "CREATE (n:Person {name: 'Alice', age: 30})"));
    assert_ok(engine.execute(&store, "g", "CREATE (n:Person {name: 'Bob', age: 20})"));

    let result = assert_ok(engine.execute(
        &store,
        "g",
        "MATCH (n:Person) FILTER n.age > 25 RETURN n.name",
    ));

    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].get("n.name"),
        Some(&Value::Str("Alice".into()))
    );
}

#[test]
fn supports_gql_filter_where_variant() {
    let engine = SedanGql::new();
    let store = fresh_store();

    assert_ok(engine.execute(&store, "g", "CREATE (n:Person {name: 'Alice', age: 30})"));

    let result = assert_ok(engine.execute(
        &store,
        "g",
        "MATCH (n:Person) FILTER WHERE n.age = 30 RETURN n.name",
    ));

    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].get("n.name"),
        Some(&Value::Str("Alice".into()))
    );
}

#[test]
fn supports_gql_mandatory_match_error_contract() {
    let engine = SedanGql::new();
    let store = fresh_store();

    let err = engine
        .execute(&store, "g", "MANDATORY MATCH (n:Ghost) RETURN n")
        .expect_err("mandatory match without rows should error");
    let message = err.to_string();
    assert!(
        message.to_lowercase().contains("mandatory"),
        "error should mention mandatory, got: {message}"
    );
}

#[test]
fn supports_gql_for_keyword() {
    let engine = SedanGql::new();
    let store = fresh_store();

    let result = assert_ok(engine.execute(&store, "g", "FOR x IN [1, 2, 3] RETURN x"));

    assert_eq!(result.rows.len(), 3);
}

#[test]
fn preserves_cypher_compatibility_after_gql_rewrite() {
    let engine = SedanGql::new();
    let store = fresh_store();

    assert_ok(engine.execute(
        &store,
        "g",
        "CREATE (a:Person {name: 'Alice'})-[:KNOWS]->(b:Person {name: 'Bob'})",
    ));

    let result = assert_ok(engine.execute(
        &store,
        "g",
        "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name",
    ));

    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].get("a.name"),
        Some(&Value::Str("Alice".into()))
    );
    assert_eq!(
        result.rows[0].get("b.name"),
        Some(&Value::Str("Bob".into()))
    );
}

#[test]
fn supports_mixed_gql_workflow_through_public_api() {
    let engine = SedanGql::new();
    let store = fresh_store();

    assert_ok(engine.execute(
        &store,
        "g",
        "INSERT (n:City {name: 'London', pop: 9000000})",
    ));
    assert_ok(engine.execute(&store, "g", "INSERT (n:City {name: 'Bath', pop: 90000})"));

    let result = assert_ok(engine.execute(
        &store,
        "g",
        "MATCH (n:City) FILTER n.pop > 1000000 RETURN n.name",
    ));

    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].get("n.name"),
        Some(&Value::Str("London".into()))
    );
}
