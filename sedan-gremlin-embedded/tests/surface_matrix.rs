use sedan_gremlin_embedded::{GraphStore, GremlinEngine, InMemoryGraphStore};

fn fresh_store(graphs: &[&str]) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    for graph in graphs {
        store.create_graph(graph).expect("create graph");
    }
    store
}

fn seed_person(store: &InMemoryGraphStore, graph: &str) {
    store
        .create_node(graph, vec!["Person".into()], Default::default())
        .expect("seed node");
}

#[test]
fn executes_representative_gremlin_workflow() {
    let engine = GremlinEngine::new();
    let store = fresh_store(&["g"]);
    seed_person(&store, "g");

    let result = engine
        .execute(&store, "g", r#"g.V().hasLabel("Person")"#)
        .expect("gremlin query should succeed");

    assert_eq!(result.rows.len(), 1);
}

#[test]
fn isolates_distinct_store_instances() {
    let engine = GremlinEngine::new();
    let store_a = fresh_store(&["g"]);
    let store_b = fresh_store(&["g"]);
    seed_person(&store_a, "g");

    let store_a_result = engine
        .execute(&store_a, "g", r#"g.V().hasLabel("Person")"#)
        .expect("store A query should succeed");
    let store_b_result = engine
        .execute(&store_b, "g", r#"g.V().hasLabel("Person")"#)
        .expect("store B query should succeed");

    assert_eq!(store_a_result.rows.len(), 1);
    assert_eq!(store_b_result.rows.len(), 0);

    let stats_a = store_a.graph_stats("g").expect("store A stats should load");
    let stats_b = store_b.graph_stats("g").expect("store B stats should load");
    assert_eq!(stats_a.node_count, 1);
    assert_eq!(stats_b.node_count, 0);
}

#[test]
fn isolates_named_graphs_within_shared_store() {
    let engine = GremlinEngine::new();
    let store = fresh_store(&["alpha", "beta"]);
    seed_person(&store, "alpha");

    let alpha_result = engine
        .execute(&store, "alpha", r#"g.V().hasLabel("Person")"#)
        .expect("alpha query should succeed");
    let beta_result = engine
        .execute(&store, "beta", r#"g.V().hasLabel("Person")"#)
        .expect("beta query should succeed");

    assert_eq!(alpha_result.rows.len(), 1);
    assert_eq!(beta_result.rows.len(), 0);

    let alpha_stats = store.graph_stats("alpha").expect("alpha stats should load");
    let beta_stats = store.graph_stats("beta").expect("beta stats should load");
    assert_eq!(alpha_stats.node_count, 1);
    assert_eq!(beta_stats.node_count, 0);
}

#[test]
fn surfaces_parse_error_prefix() {
    let engine = GremlinEngine::new();
    let store = fresh_store(&["g"]);

    let err = engine
        .execute(&store, "g", "not.a.traversal()")
        .expect_err("invalid traversal should error");
    let message = err.to_string();
    assert!(
        message.starts_with("parse error:"),
        "error should preserve the public parse prefix, got: {message}"
    );
}
