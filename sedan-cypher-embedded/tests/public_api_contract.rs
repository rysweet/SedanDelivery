use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};

fn fresh_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").expect("create graph");
    store
}

#[test]
fn executes_cypher_create_match_workflow() {
    let engine = CypherEngine::new();
    let store = fresh_store();

    engine
        .execute(&store, "g", "CREATE (p:Person {name: 'Bob'}) RETURN p.name")
        .expect("cypher create should succeed");

    let result = engine
        .execute(&store, "g", "MATCH (p:Person) RETURN p.name")
        .expect("cypher match should succeed");

    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].get("p.name"),
        Some(&Value::Str("Bob".into()))
    );
}

#[test]
fn remains_cypher_only_when_gql_surface_gains_native_keywords() {
    let engine = CypherEngine::new();
    let store = fresh_store();

    assert!(
        engine
            .execute(
                &store,
                "g",
                "INSERT (n:Person {name: 'Alice'}) RETURN n.name"
            )
            .is_err(),
        "CypherEngine should reject GQL-only INSERT syntax"
    );
}
