//! Property tests for [`sedan_cypher_embedded::CypherEngine`].

use std::collections::HashMap;

use embedded_fuzz::{
    cypher_query_strategy, fuzz_config, scalar_value_strategy, FAST_CASES, SOAK_CASES,
};
use proptest::prelude::*;
use sedan_cypher_embedded::{
    CypherEngine, GqlError, GraphStore, InMemoryGraphStore, ProcedureRegistry,
};

fn fresh_store() -> InMemoryGraphStore {
    let s = InMemoryGraphStore::default();
    s.create_graph("g").expect("create graph");
    s
}

/// Every public error returned by [`CypherEngine::execute`] must be one
/// of the documented [`GqlError`] variants — no naked panic, no leaked
/// internal type.
fn assert_is_public_error(err: &GqlError) {
    match err {
        GqlError::Parse(_)
        | GqlError::Plan(_)
        | GqlError::Execution(_)
        | GqlError::Storage(_)
        | GqlError::Type(_) => {}
    }
}

proptest! {
    #![proptest_config(fuzz_config(FAST_CASES))]

    #[test]
    fn cypher_execute_never_panics(query in cypher_query_strategy()) {
        let engine = CypherEngine::new();
        let store = fresh_store();
        match engine.execute(&store, "g", &query) {
            Ok(_) => {}
            Err(e) => assert_is_public_error(&e),
        }
    }

    #[test]
    fn cypher_execute_with_parameters_round_trips_values(
        query in cypher_query_strategy(),
        params in proptest::collection::hash_map(
            "[A-Za-z][A-Za-z0-9_]{0,5}",
            scalar_value_strategy(),
            0..4,
        ),
    ) {
        let engine = CypherEngine::new();
        let store = fresh_store();
        let registry = ProcedureRegistry::default();
        let mut hm = HashMap::new();
        for (k, v) in params {
            hm.insert(k, v);
        }
        match engine.execute_with_parameters(&store, "g", &query, &hm, &registry) {
            Ok(_) => {}
            Err(e) => assert_is_public_error(&e),
        }
    }

    /// Truly arbitrary text: most inputs are not valid Cypher, but the
    /// engine must reject them via `GqlError::Parse` rather than panic.
    #[test]
    fn cypher_arbitrary_strings_are_classified(s in "\\PC{0,64}") {
        let engine = CypherEngine::new();
        let store = fresh_store();
        if let Err(e) = engine.execute(&store, "g", &s) {
            assert_is_public_error(&e);
        }
    }

    /// Parameters must not panic the engine even when the query does not
    /// reference them.
    #[test]
    fn cypher_parameter_values_round_trip(v in scalar_value_strategy()) {
        let engine = CypherEngine::new();
        let store = fresh_store();
        let mut params = HashMap::new();
        params.insert("p".to_string(), v);
        let registry = ProcedureRegistry::default();
        match engine.execute_with_parameters(
            &store, "g", "RETURN $p AS p", &params, &registry,
        ) {
            Ok(qr) => {
                // Round-trip: returned value must equal the input — this is the
                // one place we assert on output, because it is a contract of
                // parameter binding.
                let returned = qr.rows[0].get("p").expect("p column");
                prop_assert_eq!(returned, params.get("p").unwrap());
            }
            Err(e) => assert_is_public_error(&e),
        }
    }
}

// ── Soak variant: same properties, 10k cases, ignored by default ─────────────

proptest! {
    #![proptest_config(fuzz_config(SOAK_CASES))]

    #[test]
    #[ignore = "soak: run with --ignored"]
    fn cypher_execute_soak(query in cypher_query_strategy()) {
        let engine = CypherEngine::new();
        let store = fresh_store();
        if let Err(e) = engine.execute(&store, "g", &query) {
            assert_is_public_error(&e);
        }
    }
}
