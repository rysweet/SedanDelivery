use sedan_gremlin_embedded::{GraphStore, GremlinEngine, InMemoryGraphStore};

fn fresh_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").expect("create graph");
    store
}

#[test]
fn executes_text_traversal_against_shared_store() {
    let engine = GremlinEngine::new();
    let store = fresh_store();

    store
        .create_node("g", vec!["Person".into()], Default::default())
        .expect("seed node");

    let result = engine
        .execute(&store, "g", "g.V().count()")
        .expect("gremlin query should succeed");

    assert_eq!(result.rows.len(), 1);
}
