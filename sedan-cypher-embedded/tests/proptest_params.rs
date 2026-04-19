//! Property-based round-trip tests for parameter values.
//!
//! Generates arbitrary Cypher [`Value`]s and asserts that
//! `RETURN $x AS x` echoes the same value back through the engine.
//! Floats are constrained to non-NaN / non-subnormal so equality is
//! well-defined; map keys avoid `$` to keep things parser-friendly.

use std::collections::HashMap;

use indexmap::IndexMap;
use proptest::collection::{hash_map, vec};
use proptest::prelude::*;
use proptest::test_runner::Config;
use sedan_cypher_embedded::{
    CypherEngine, GraphStore, InMemoryGraphStore, ProcedureRegistry, Value,
};

fn finite_f64() -> impl Strategy<Value = f64> {
    // Non-NaN, finite, not subnormal — safe for round-trip equality.
    proptest::num::f64::POSITIVE | proptest::num::f64::NEGATIVE | proptest::num::f64::ZERO
}

fn ascii_key() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9_]{0,8}"
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
    leaf.prop_recursive(3, 16, 6, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..5).prop_map(Value::List),
            hash_map(ascii_key(), inner, 0..5).prop_map(|m| {
                let mut im: IndexMap<Box<str>, Value> = IndexMap::new();
                let mut entries: Vec<_> = m.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                for (k, v) in entries {
                    im.insert(k.into_boxed_str(), v);
                }
                Value::Map(im)
            }),
        ]
    })
}

proptest! {
    #![proptest_config(Config { cases: 64, .. Config::default() })]

    #[test]
    fn return_param_round_trip(v in arb_value()) {
        let engine = CypherEngine::new();
        let store = InMemoryGraphStore::default();
        store.create_graph("g").unwrap();
        let registry = ProcedureRegistry::new();
        let mut params = HashMap::new();
        params.insert("x".to_string(), v.clone());
        let result = engine
            .execute_with_parameters(&store, "g", "RETURN $x AS x", &params, &registry)
            .expect("RETURN $x must succeed");
        prop_assert_eq!(result.rows.len(), 1);
        prop_assert_eq!(result.rows[0].get("x"), Some(&v));
    }
}
