//! Mutation-gap test #02.
//!
//! cargo-mutants identified that `CypherEngine::clear_cache` could be
//! replaced with a no-op `()` and no test would notice. This file pins the
//! clear-actually-clears behavior so that mutation is caught.
//!
//! See: docs/MUTATION_TESTING.md

use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore};

fn test_store() -> InMemoryGraphStore {
    let s = InMemoryGraphStore::default();
    s.create_graph("g").unwrap();
    s
}

#[test]
fn clear_cache_resets_observable_cache_state() {
    let engine = CypherEngine::new();
    let store = test_store();

    engine.execute(&store, "g", "RETURN 1 AS n").unwrap();
    engine.execute(&store, "g", "RETURN 2 AS n").unwrap();
    let before = engine.cache_len();
    assert!(
        before >= 2,
        "expected cache to hold both queries before clear, got cache_len = {before}"
    );

    engine.clear_cache();

    // If clear_cache were replaced with `()`, the cache would still hold
    // the two plans inserted above.
    assert_eq!(
        engine.cache_len(),
        0,
        "clear_cache must drop all cached plans; cache_len = {} after clear \
         suggests clear_cache was replaced with a no-op",
        engine.cache_len()
    );
}

#[test]
fn clear_cache_then_repopulates() {
    let engine = CypherEngine::new();
    let store = test_store();

    engine.execute(&store, "g", "RETURN 1 AS n").unwrap();
    engine.clear_cache();
    assert_eq!(engine.cache_len(), 0);

    engine.execute(&store, "g", "RETURN 2 AS n").unwrap();
    assert_eq!(
        engine.cache_len(),
        1,
        "engine must continue caching plans after clear_cache; cache_len = {}",
        engine.cache_len()
    );
}
