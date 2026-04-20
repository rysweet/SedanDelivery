//! Property tests for [`sedan_gremlin_embedded::GremlinEngine`].

use embedded_fuzz::{fuzz_config, gremlin_query_strategy, FAST_CASES, SOAK_CASES};
use proptest::prelude::*;
use sedan_gremlin_embedded::{
    ExecutionBudget, GraphStore, GremlinEngine, GremlinError, InMemoryGraphStore,
};

fn fresh_store() -> InMemoryGraphStore {
    let s = InMemoryGraphStore::default();
    s.create_graph("g").expect("create graph");
    s
}

fn assert_is_public_error(err: &GremlinError) {
    match err {
        GremlinError::Parse(_) | GremlinError::Compile(_) | GremlinError::Execution(_) => {}
    }
}

proptest! {
    #![proptest_config(fuzz_config(FAST_CASES))]

    #[test]
    fn gremlin_execute_never_panics(query in gremlin_query_strategy()) {
        let engine = GremlinEngine::new();
        let store = fresh_store();
        if let Err(e) = engine.execute(&store, "g", &query) {
            assert_is_public_error(&e);
        }
    }

    #[test]
    fn gremlin_execute_with_budget_never_panics(query in gremlin_query_strategy()) {
        let engine = GremlinEngine::new();
        let store = fresh_store();
        let mut budget = ExecutionBudget::default_budget();
        if let Err(e) = engine.execute_with_budget(&store, "g", &query, &mut budget) {
            assert_is_public_error(&e);
        }
    }

    #[test]
    fn gremlin_arbitrary_strings_are_classified(s in "\\PC{0,64}") {
        let engine = GremlinEngine::new();
        let store = fresh_store();
        if let Err(e) = engine.execute(&store, "g", &s) {
            assert_is_public_error(&e);
        }
    }
}

proptest! {
    #![proptest_config(fuzz_config(SOAK_CASES))]

    #[test]
    #[ignore = "soak: run with --ignored"]
    fn gremlin_execute_soak(query in gremlin_query_strategy()) {
        let engine = GremlinEngine::new();
        let store = fresh_store();
        if let Err(e) = engine.execute(&store, "g", &query) {
            assert_is_public_error(&e);
        }
    }
}
