use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};

fn fresh_store(graphs: &[&str]) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    for graph in graphs {
        store.create_graph(graph).expect("create graph");
    }
    store
}

#[test]
fn executes_representative_cypher_workflow() {
    let engine = CypherEngine::new();
    let store = fresh_store(&["g"]);

    let created = engine
        .execute(
            &store,
            "g",
            "CREATE (p:Person {name: 'Alice'}) RETURN p.name",
        )
        .expect("cypher create should succeed");
    assert_eq!(created.rows.len(), 1);
    assert_eq!(
        created.rows[0].get("p.name"),
        Some(&Value::Str("Alice".into()))
    );

    let matched = engine
        .execute(&store, "g", "MATCH (p:Person) RETURN p.name")
        .expect("cypher match should succeed");
    assert_eq!(matched.rows.len(), 1);
    assert_eq!(
        matched.rows[0].get("p.name"),
        Some(&Value::Str("Alice".into()))
    );
}

#[test]
fn isolates_distinct_store_instances() {
    let engine = CypherEngine::new();
    let store_a = fresh_store(&["g"]);
    let store_b = fresh_store(&["g"]);

    engine
        .execute(&store_a, "g", "CREATE (p:Person {name: 'Alice'})")
        .expect("store A write should succeed");

    let store_b_result = engine
        .execute(&store_b, "g", "MATCH (p:Person) RETURN p.name")
        .expect("store B read should succeed");
    assert_eq!(store_b_result.rows.len(), 0);

    let stats_a = store_a.graph_stats("g").expect("store A stats should load");
    let stats_b = store_b.graph_stats("g").expect("store B stats should load");
    assert_eq!(stats_a.node_count, 1);
    assert_eq!(stats_b.node_count, 0);
}

#[test]
fn isolates_named_graphs_within_shared_store() {
    let engine = CypherEngine::new();
    let store = fresh_store(&["alpha", "beta"]);

    engine
        .execute(&store, "alpha", "CREATE (p:Person {name: 'Alice'})")
        .expect("alpha write should succeed");

    let beta_result = engine
        .execute(&store, "beta", "MATCH (p:Person) RETURN p.name")
        .expect("beta read should succeed");
    assert_eq!(beta_result.rows.len(), 0);

    let alpha_stats = store.graph_stats("alpha").expect("alpha stats should load");
    let beta_stats = store.graph_stats("beta").expect("beta stats should load");
    assert_eq!(alpha_stats.node_count, 1);
    assert_eq!(beta_stats.node_count, 0);
}

#[test]
fn surfaces_stable_public_error_for_missing_graph() {
    let engine = CypherEngine::new();
    let store = fresh_store(&["g"]);

    let err = engine
        .execute(&store, "missing", "RETURN 1 AS n")
        .expect_err("missing graph should error");
    let message = err.to_string();

    assert!(
        message.contains("storage error:"),
        "error should preserve the public storage prefix, got: {message}"
    );
    assert!(
        message.contains("does not exist"),
        "error should describe the missing graph, got: {message}"
    );
}
