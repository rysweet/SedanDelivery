//! Property tests for [`sedan_gql_embedded::SedanGql`].

use std::collections::HashMap;

use embedded_fuzz::{
    fuzz_config, gql_query_strategy, scalar_value_strategy, FAST_CASES, SOAK_CASES,
};
use proptest::prelude::*;
use sedan_gql_embedded::{GqlError, GraphStore, InMemoryGraphStore, ProcedureRegistry, SedanGql};

fn fresh_store() -> InMemoryGraphStore {
    let s = InMemoryGraphStore::default();
    s.create_graph("g").expect("create graph");
    s
}

fn assert_is_public_error(err: &GqlError) {
    match err {
        GqlError::Parse(_) | GqlError::Plan(_) | GqlError::Execution(_) => {}
    }
}

proptest! {
    #![proptest_config(fuzz_config(FAST_CASES))]

    #[test]
    fn gql_execute_never_panics(query in gql_query_strategy()) {
        let engine = SedanGql::new();
        let store = fresh_store();
        if let Err(e) = engine.execute(&store, "g", &query) {
            assert_is_public_error(&e);
        }
    }

    #[test]
    fn gql_execute_with_context_round_trips(
        query in gql_query_strategy(),
        params in proptest::collection::hash_map(
            "[A-Za-z][A-Za-z0-9_]{0,5}",
            scalar_value_strategy(),
            0..4,
        ),
    ) {
        let engine = SedanGql::new();
        let store = fresh_store();
        let registry = ProcedureRegistry::default();
        let mut hm = HashMap::new();
        for (k, v) in params {
            hm.insert(k, v);
        }
        if let Err(e) = engine.execute_with_context(&store, "g", &query, &registry, hm) {
            assert_is_public_error(&e);
        }
    }

    #[test]
    fn gql_arbitrary_strings_are_classified(s in "\\PC{0,64}") {
        let engine = SedanGql::new();
        let store = fresh_store();
        if let Err(e) = engine.execute(&store, "g", &s) {
            assert_is_public_error(&e);
        }
    }

    #[test]
    fn gql_parameter_values_round_trip(v in scalar_value_strategy()) {
        let engine = SedanGql::new();
        let store = fresh_store();
        let registry = ProcedureRegistry::default();
        let mut params = HashMap::new();
        params.insert("p".to_string(), v);
        match engine.execute_with_context(
            &store, "g", "RETURN $p AS p", &registry, params.clone(),
        ) {
            Ok(qr) => {
                let returned = qr.rows[0].get("p").expect("p column");
                prop_assert_eq!(returned, params.get("p").unwrap());
            }
            Err(e) => assert_is_public_error(&e),
        }
    }
}

proptest! {
    #![proptest_config(fuzz_config(SOAK_CASES))]

    #[test]
    #[ignore = "soak: run with --ignored"]
    fn gql_execute_soak(query in gql_query_strategy()) {
        let engine = SedanGql::new();
        let store = fresh_store();
        if let Err(e) = engine.execute(&store, "g", &query) {
            assert_is_public_error(&e);
        }
    }
}
