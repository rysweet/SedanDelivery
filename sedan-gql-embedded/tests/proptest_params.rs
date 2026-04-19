//! Property-based round-trip tests for GQL parameter values.
//!
//! Mirrors `sedan-cypher-embedded/tests/proptest_params.rs` but exercises the
//! GQL surface (`SedanGql::execute_with_context`). Generates arbitrary
//! [`Value`]s and asserts `RETURN $x AS x` echoes them back.

use std::collections::HashMap;

use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::Config;
use sedan_gql_embedded::prelude::*;

fn finite_f64() -> impl Strategy<Value = f64> {
    proptest::num::f64::POSITIVE | proptest::num::f64::NEGATIVE | proptest::num::f64::ZERO
}

fn ascii_str() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 _-]{0,16}"
}

fn arb_scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::Int),
        finite_f64().prop_map(Value::Float),
        ascii_str().prop_map(|s| Value::Str(s.into_boxed_str())),
    ]
}

fn arb_value() -> impl Strategy<Value = Value> {
    let leaf = arb_scalar();
    leaf.prop_recursive(2, 8, 4, |inner| vec(inner, 0..4).prop_map(Value::List))
}

proptest! {
    #![proptest_config(Config { cases: 64, .. Config::default() })]

    #[test]
    fn return_param_round_trip(v in arb_value()) {
        let engine = SedanGql::new();
        let store = InMemoryGraphStore::default();
        store.create_graph("g").unwrap();
        let registry = ProcedureRegistry::default();
        let mut params = HashMap::new();
        params.insert("x".to_string(), v.clone());
        let result = engine
            .execute_with_context(&store, "g", "RETURN $x AS x", &registry, params)
            .expect("RETURN $x must succeed");
        prop_assert_eq!(result.rows.len(), 1);
        prop_assert_eq!(result.rows[0].get("x"), Some(&v));
    }
}
