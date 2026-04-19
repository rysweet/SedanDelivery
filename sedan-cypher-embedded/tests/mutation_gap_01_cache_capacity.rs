//! Mutation-gap test #01.
//!
//! cargo-mutants identified that
//! `CypherEngine::with_cache_capacity` could be replaced with
//! `Default::default()` (which is `CypherEngine::new()` — capacity 128) and
//! no test would notice. This file pins the capacity-honoring behavior so
//! that mutation is caught.
//!
//! See: docs/MUTATION_TESTING.md

use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore};

fn test_store() -> InMemoryGraphStore {
    let s = InMemoryGraphStore::default();
    s.create_graph("g").unwrap();
    s
}

#[test]
fn capacity_zero_disables_caching_observably() {
    let engine = CypherEngine::with_cache_capacity(0);
    let store = test_store();
    for i in 0..5 {
        let q = format!("RETURN {i} AS n");
        engine.execute(&store, "g", &q).unwrap();
    }
    // PlanCache with capacity 0 evicts on every insert, so cache_len never
    // exceeds 1. If the constructor were replaced with Default::default()
    // (capacity 128) the cache would hold all 5 plans.
    assert!(
        engine.cache_len() <= 1,
        "with_cache_capacity(0) must produce a non-caching engine; \
         cache_len = {} (>1) suggests the capacity argument was ignored",
        engine.cache_len()
    );
}

#[test]
fn capacity_two_evicts_observably() {
    let engine = CypherEngine::with_cache_capacity(2);
    let store = test_store();
    for i in 0..6 {
        let q = format!("RETURN {i} AS n");
        engine.execute(&store, "g", &q).unwrap();
    }
    // A cache of 128 (the default-capacity mutant) would retain all 6 plans;
    // a cache of 2 must hold at most 2.
    assert!(
        engine.cache_len() <= 2,
        "with_cache_capacity(2) must cap cache at 2 entries; cache_len = {} \
         suggests the capacity argument was ignored",
        engine.cache_len()
    );
}
