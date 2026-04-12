use sedan_gql_embedded::{GraphStore, InMemoryGraphStore, QueryResult, SedanGql, Value};

fn fresh_store(graphs: &[&str]) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    for graph in graphs {
        store.create_graph(graph).expect("create graph");
    }
    store
}

fn assert_ok(result: Result<QueryResult, sedan_gql_embedded::GqlError>) -> QueryResult {
    result.expect("query should succeed")
}

#[test]
fn executes_representative_gql_workflow() {
    let engine = SedanGql::new();
    let store = fresh_store(&["g"]);

    assert_ok(engine.execute(
        &store,
        "g",
        "INSERT (n:City {name: 'London', pop: 9000000})",
    ));
    assert_ok(engine.execute(&store, "g", "INSERT (n:City {name: 'Bath', pop: 90000})"));

    let matched = assert_ok(engine.execute(
        &store,
        "g",
        "MATCH (n:City) FILTER n.pop > 1000000 RETURN n.name",
    ));
    assert_eq!(matched.rows.len(), 1);
    assert_eq!(
        matched.rows[0].get("n.name"),
        Some(&Value::Str("London".into()))
    );
}

#[test]
fn isolates_distinct_store_instances() {
    let engine = SedanGql::new();
    let store_a = fresh_store(&["g"]);
    let store_b = fresh_store(&["g"]);

    assert_ok(engine.execute(&store_a, "g", "INSERT (n:Person {name: 'Alice'})"));

    let store_b_result = assert_ok(engine.execute(&store_b, "g", "MATCH (n:Person) RETURN n.name"));
    assert_eq!(store_b_result.rows.len(), 0);

    let stats_a = store_a.graph_stats("g").expect("store A stats should load");
    let stats_b = store_b.graph_stats("g").expect("store B stats should load");
    assert_eq!(stats_a.node_count, 1);
    assert_eq!(stats_b.node_count, 0);
}

#[test]
fn isolates_named_graphs_within_shared_store() {
    let engine = SedanGql::new();
    let store = fresh_store(&["alpha", "beta"]);

    assert_ok(engine.execute(&store, "alpha", "INSERT (n:Person {name: 'Alice'})"));

    let beta_result = assert_ok(engine.execute(&store, "beta", "MATCH (n:Person) RETURN n.name"));
    assert_eq!(beta_result.rows.len(), 0);

    let alpha_stats = store.graph_stats("alpha").expect("alpha stats should load");
    let beta_stats = store.graph_stats("beta").expect("beta stats should load");
    assert_eq!(alpha_stats.node_count, 1);
    assert_eq!(beta_stats.node_count, 0);
}

#[test]
fn surfaces_mandatory_match_error_contract() {
    let engine = SedanGql::new();
    let store = fresh_store(&["g"]);

    let err = engine
        .execute(&store, "g", "MANDATORY MATCH (n:Ghost) RETURN n")
        .expect_err("mandatory match without rows should error");
    let message = err.to_string();
    assert!(
        message.to_lowercase().contains("mandatory"),
        "error should mention mandatory, got: {message}"
    );
}
