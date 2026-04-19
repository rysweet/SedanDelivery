//! Tests for bytecode deserialization (basic).
#![cfg(test)]

use super::deserialize::json_to_gremlin_value;
use super::*;

#[test]
fn deserialize_simple_traversal() {
    let json: serde_json::Value = serde_json::json!({
        "step": [
            ["V"],
            ["has", "name", "marko"],
            ["out", "knows"],
            ["values", "name"]
        ]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps.len(), 4);
    assert_eq!(steps[0], GremlinStep::V(None));
    assert_eq!(
        steps[1],
        GremlinStep::Has("name".into(), Some(GremlinValue::String("marko".into())))
    );
    assert_eq!(steps[2], GremlinStep::Out(vec!["knows".into()]));
    assert_eq!(steps[3], GremlinStep::Values(vec!["name".into()]));
}

#[test]
fn deserialize_v_with_ids() {
    let json: serde_json::Value = serde_json::json!({
        "step": [["V", 1, 2, 3]]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::V(Some(vec![
            GremlinValue::Integer(1),
            GremlinValue::Integer(2),
            GremlinValue::Integer(3),
        ]))
    );
}

#[test]
fn deserialize_mutation_steps() {
    let json: serde_json::Value = serde_json::json!({
        "step": [
            ["addV", "person"],
            ["property", "name", "alice"],
            ["property", "age", 30]
        ]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0], GremlinStep::AddV("person".into()));
    assert_eq!(
        steps[1],
        GremlinStep::Property("name".into(), GremlinValue::String("alice".into()))
    );
    assert_eq!(
        steps[2],
        GremlinStep::Property("age".into(), GremlinValue::Integer(30))
    );
}

#[test]
fn unknown_step_returns_error() {
    let json: serde_json::Value = serde_json::json!({
        "step": [["nonexistent_step"]]
    });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::UnknownStep(_)));
}

#[test]
fn deserialize_limit_and_dedup() {
    let json: serde_json::Value = serde_json::json!({
        "step": [
            ["V"],
            ["dedup"],
            ["limit", 10]
        ]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[1], GremlinStep::Dedup(vec![]));
    assert_eq!(steps[2], GremlinStep::Limit(10));
}

// ── Source step coverage ────────────────────────────────────────────────

#[test]
fn deserialize_e_no_args() {
    let json = serde_json::json!({ "step": [["E"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::E(None));
}

#[test]
fn deserialize_e_with_ids() {
    let json = serde_json::json!({ "step": [["E", 10, 20]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::E(Some(vec![
            GremlinValue::Integer(10),
            GremlinValue::Integer(20),
        ]))
    );
}

// ── Filter step coverage ────────────────────────────────────────────────

#[test]
fn deserialize_has_label() {
    let json = serde_json::json!({ "step": [["hasLabel", "Person", "Animal"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::HasLabel(vec!["Person".into(), "Animal".into()])
    );
}

#[test]
fn deserialize_has_key_only() {
    let json = serde_json::json!({ "step": [["has", "age"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Has("age".into(), None));
}

#[test]
fn deserialize_has_id() {
    let json = serde_json::json!({ "step": [["hasId", 1, 2]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::HasId(vec![GremlinValue::Integer(1), GremlinValue::Integer(2),])
    );
}

#[test]
fn deserialize_has_missing_key_returns_error() {
    let json = serde_json::json!({ "step": [["has"]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::MissingArgument(_)));
}

// ── Traversal step coverage ─────────────────────────────────────────────

#[test]
fn deserialize_in_both_edge_steps() {
    let json = serde_json::json!({
        "step": [
            ["in", "knows"],
            ["both", "likes", "follows"],
            ["outE", "created"],
            ["inE"],
            ["bothE"],
            ["inV"],
            ["outV"],
            ["bothV"],
            ["otherV"]
        ]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::In(vec!["knows".into()]));
    assert_eq!(
        steps[1],
        GremlinStep::Both(vec!["likes".into(), "follows".into()])
    );
    assert_eq!(steps[2], GremlinStep::OutE(vec!["created".into()]));
    assert_eq!(steps[3], GremlinStep::InE(vec![]));
    assert_eq!(steps[4], GremlinStep::BothE(vec![]));
    assert_eq!(steps[5], GremlinStep::InV);
    assert_eq!(steps[6], GremlinStep::OutV);
    assert_eq!(steps[7], GremlinStep::BothV);
    assert_eq!(steps[8], GremlinStep::OtherV);
}

// ── Element step coverage ───────────────────────────────────────────────

#[test]
fn deserialize_element_steps() {
    let json = serde_json::json!({
        "step": [
            ["values", "name", "age"],
            ["valueMap"],
            ["properties", "name"],
            ["id"],
            ["label"]
        ]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::Values(vec!["name".into(), "age".into()])
    );
    assert_eq!(steps[1], GremlinStep::ValueMap(vec![], 0));
    assert_eq!(steps[2], GremlinStep::Properties(vec!["name".into()]));
    assert_eq!(steps[3], GremlinStep::Id);
    assert_eq!(steps[4], GremlinStep::Label);
}

// ── Map/select step coverage ────────────────────────────────────────────

#[test]
fn deserialize_select_and_as() {
    let json = serde_json::json!({
        "step": [
            ["as", "a"],
            ["select", "a", "b"]
        ]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::As("a".into()));
    assert_eq!(
        steps[1],
        GremlinStep::Select(None, vec!["a".into(), "b".into()])
    );
}

#[test]
fn deserialize_as_missing_arg_returns_error() {
    let json = serde_json::json!({ "step": [["as"]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::MissingArgument(_)));
}

// ── Aggregation step coverage ───────────────────────────────────────────

#[test]
fn deserialize_aggregation_steps() {
    let json = serde_json::json!({
        "step": [
            ["count"],
            ["sum"],
            ["min"],
            ["max"],
            ["mean"],
            ["group"],
            ["groupCount"],
            ["fold"],
            ["unfold"]
        ]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Count);
    assert_eq!(steps[1], GremlinStep::Sum);
    assert_eq!(steps[2], GremlinStep::Min);
    assert_eq!(steps[3], GremlinStep::Max);
    assert_eq!(steps[4], GremlinStep::Mean);
    assert_eq!(steps[5], GremlinStep::Group(None));
    assert_eq!(steps[6], GremlinStep::GroupCount(None));
    assert_eq!(steps[7], GremlinStep::Fold);
    assert_eq!(steps[8], GremlinStep::Unfold);
}

// ── Ordering & paging step coverage ─────────────────────────────────────

#[test]
fn deserialize_order() {
    let json = serde_json::json!({ "step": [["order"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Order);
}

#[test]
fn deserialize_range() {
    let json = serde_json::json!({ "step": [["range", 2, 5]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Range(2, 5));
}

#[test]
fn deserialize_range_missing_arg_returns_error() {
    let json = serde_json::json!({ "step": [["range", 2]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::MissingArgument(_)));
}

#[test]
fn deserialize_limit_missing_arg_returns_error() {
    let json = serde_json::json!({ "step": [["limit"]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::MissingArgument(_)));
}

// ── Mutation step coverage ──────────────────────────────────────────────

#[test]
fn deserialize_add_e() {
    let json = serde_json::json!({ "step": [["addE", "knows"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::AddE("knows".into()));
}

#[test]
fn deserialize_add_e_missing_label_returns_error() {
    let json = serde_json::json!({ "step": [["addE"]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::MissingArgument(_)));
}

#[test]
fn deserialize_add_v_missing_label_returns_error() {
    let json = serde_json::json!({ "step": [["addV"]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::MissingArgument(_)));
}

#[test]
fn deserialize_drop() {
    let json = serde_json::json!({ "step": [["drop"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Drop);
}

#[test]
fn deserialize_property_no_value() {
    let json = serde_json::json!({ "step": [["property", "key"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::Property("key".into(), GremlinValue::Null)
    );
}

#[test]
fn deserialize_property_missing_key_returns_error() {
    let json = serde_json::json!({ "step": [["property"]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::MissingArgument(_)));
}

// ── Misc step coverage ──────────────────────────────────────────────────

#[test]
fn deserialize_path_and_identity() {
    let json = serde_json::json!({ "step": [["path"], ["identity"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Path);
    assert_eq!(steps[1], GremlinStep::Identity);
}

#[test]
fn deserialize_times() {
    let json = serde_json::json!({ "step": [["times", 3]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Times(3));
}

#[test]
fn deserialize_times_missing_arg_returns_error() {
    let json = serde_json::json!({ "step": [["times"]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::MissingArgument(_)));
}

#[test]
fn deserialize_emit() {
    let json = serde_json::json!({ "step": [["emit"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Emit);
}

#[test]
fn deserialize_aggregate() {
    let json = serde_json::json!({ "step": [["aggregate", "x"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Aggregate("x".into()));
}

#[test]
fn deserialize_aggregate_missing_key_returns_error() {
    let json = serde_json::json!({ "step": [["aggregate"]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::MissingArgument(_)));
}

#[test]
fn deserialize_cap() {
    let json = serde_json::json!({ "step": [["cap", "x", "y"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Cap(vec!["x".into(), "y".into()]));
}

#[test]
fn deserialize_constant_and_inject() {
    let json = serde_json::json!({
        "step": [
            ["constant", "hello"],
            ["inject", 1, 2, 3]
        ]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::Constant(GremlinValue::String("hello".into()))
    );
    assert_eq!(
        steps[1],
        GremlinStep::Inject(vec![
            GremlinValue::Integer(1),
            GremlinValue::Integer(2),
            GremlinValue::Integer(3),
        ])
    );
}

#[test]
fn deserialize_project() {
    let json = serde_json::json!({ "step": [["project", "a", "b"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Project(vec!["a".into(), "b".into()]));
}

#[test]
fn deserialize_tail_default_and_explicit() {
    let json = serde_json::json!({ "step": [["tail"], ["tail", 5]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Tail(1));
    assert_eq!(steps[1], GremlinStep::Tail(5));
}

#[test]
fn deserialize_skip() {
    let json = serde_json::json!({ "step": [["skip", 3]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Skip(3));
}

#[test]
fn deserialize_skip_missing_arg_returns_error() {
    let json = serde_json::json!({ "step": [["skip"]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::MissingArgument(_)));
}

#[test]
fn deserialize_element_map() {
    let json = serde_json::json!({ "step": [["elementMap", "name"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::ElementMap(vec!["name".into()]));
}

#[test]
fn deserialize_has_key_and_has_value() {
    let json = serde_json::json!({
        "step": [
            ["hasKey", "name", "age"],
            ["hasValue", 42, "hello"]
        ]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::HasKey(vec!["name".into(), "age".into()])
    );
    assert_eq!(
        steps[1],
        GremlinStep::HasValue(vec![
            GremlinValue::Integer(42),
            GremlinValue::String("hello".into()),
        ])
    );
}

#[test]
fn deserialize_simple_and_cyclic_path() {
    let json = serde_json::json!({ "step": [["simplePath"], ["cyclicPath"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::SimplePath);
    assert_eq!(steps[1], GremlinStep::CyclicPath);
}

#[test]
fn deserialize_coin() {
    let json = serde_json::json!({ "step": [["coin", 0.5]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Coin(0.5));
}

#[test]
fn deserialize_coin_missing_arg_returns_error() {
    let json = serde_json::json!({ "step": [["coin"]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::MissingArgument(_)));
}

#[test]
fn deserialize_none_and_barrier() {
    let json = serde_json::json!({ "step": [["none"], ["barrier"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::None);
    assert_eq!(steps[1], GremlinStep::Barrier);
}

#[test]
fn deserialize_barrier_norm_sack() {
    let json = serde_json::json!({ "step": [["barrier", "Barrier.normSack"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::BarrierNormSack);
}

#[test]
fn deserialize_sample() {
    let json = serde_json::json!({ "step": [["sample", 5]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Sample(5));
}

#[test]
fn deserialize_sample_missing_arg_returns_error() {
    let json = serde_json::json!({ "step": [["sample"]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::MissingArgument(_)));
}

#[test]
fn deserialize_discard_loops_sack() {
    let json = serde_json::json!({ "step": [["discard"], ["loops"], ["sack"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Discard);
    assert_eq!(steps[1], GremlinStep::Loops(None));
    assert_eq!(steps[2], GremlinStep::Sack);
}

#[test]
fn deserialize_loops_with_label() {
    let json = serde_json::json!({ "step": [["loops", "outer"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::Loops(Some("outer".into())));
}

#[test]
fn deserialize_repeat_with_inner_bytecode() {
    let json = serde_json::json!({
        "step": [["repeat", { "step": [["out"]] }], ["times", 3]]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::Repeat(None, vec![GremlinStep::Out(vec![])])
    );
    assert_eq!(steps[1], GremlinStep::Times(3));
}

#[test]
fn deserialize_repeat_with_graphson_wrapped_inner() {
    // Standard TinkerPop GraphSON wraps nested traversals in @type/@value.
    let json = serde_json::json!({
        "step": [[
            "repeat",
            { "@type": "g:Bytecode", "@value": { "step": [["out"]] } }
        ]]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::Repeat(None, vec![GremlinStep::Out(vec![])])
    );
}

#[test]
fn deserialize_repeat_with_label() {
    let json = serde_json::json!({
        "step": [["repeat", "outer", { "step": [["out"]] }]]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::Repeat(Some("outer".into()), vec![GremlinStep::Out(vec![])])
    );
}

// ── Predicate parsing coverage ──────────────────────────────────────────

#[test]
fn deserialize_is_with_scalar() {
    let json = serde_json::json!({ "step": [["is", 42]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::Is(Predicate::Eq(GremlinValue::Integer(42)))
    );
}

#[test]
fn deserialize_is_with_gt_predicate() {
    let json = serde_json::json!({
        "step": [["is", { "@type": "g:P", "@value": { "predicate": "gt", "value": 10 } }]]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::Is(Predicate::Gt(GremlinValue::Integer(10)))
    );
}

#[test]
fn deserialize_is_with_all_comparison_predicates() {
    for (pred_name, expected_ctor) in [
        ("eq", Predicate::Eq(GremlinValue::Integer(5)) as Predicate),
        ("neq", Predicate::Neq(GremlinValue::Integer(5))),
        ("gte", Predicate::Gte(GremlinValue::Integer(5))),
        ("lt", Predicate::Lt(GremlinValue::Integer(5))),
        ("lte", Predicate::Lte(GremlinValue::Integer(5))),
    ] {
        let json = serde_json::json!({
            "step": [["is", { "@type": "g:P", "@value": { "predicate": pred_name, "value": 5 } }]]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Is(expected_ctor),
            "failed for predicate: {pred_name}"
        );
    }
}

#[test]
fn deserialize_is_with_within_list() {
    let json = serde_json::json!({
        "step": [["is", { "@type": "g:P", "@value": { "predicate": "within", "value": [1, 2, 3] } }]]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::Is(Predicate::Within(vec![
            GremlinValue::Integer(1),
            GremlinValue::Integer(2),
            GremlinValue::Integer(3),
        ]))
    );
}

#[test]
fn deserialize_is_with_without_scalar() {
    let json = serde_json::json!({
        "step": [["is", { "@type": "g:P", "@value": { "predicate": "without", "value": "x" } }]]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::Is(Predicate::Without(vec![GremlinValue::String("x".into())]))
    );
}

#[test]
fn deserialize_is_with_between() {
    let json = serde_json::json!({
        "step": [["is", { "@type": "g:P", "@value": { "predicate": "between", "value": 1, "other": 10 } }]]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::Is(Predicate::Between(
            GremlinValue::Integer(1),
            GremlinValue::Integer(10),
        ))
    );
}

#[test]
fn deserialize_is_missing_arg_returns_error() {
    let json = serde_json::json!({ "step": [["is"]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::MissingArgument(_)));
}

#[test]
fn deserialize_is_unknown_predicate_returns_error() {
    let json = serde_json::json!({
        "step": [["is", { "@type": "g:P", "@value": { "predicate": "bogus", "value": 1 } }]]
    });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::UnknownStep(_)));
}

// ── Structural / edge-case coverage ─────────────────────────────────────

#[test]
fn deserialize_empty_step_array_skipped() {
    let json = serde_json::json!({ "step": [[], ["V"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0], GremlinStep::V(None));
}

#[test]
fn deserialize_missing_step_key_returns_error() {
    let json = serde_json::json!({ "data": [["V"]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::InvalidArgument { .. }));
}

#[test]
fn deserialize_non_array_entry_returns_error() {
    let json = serde_json::json!({ "step": ["not_an_array"] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::InvalidArgument { .. }));
}

#[test]
fn deserialize_non_string_step_name_returns_error() {
    let json = serde_json::json!({ "step": [[42]] });
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::InvalidArgument { .. }));
}

#[test]
fn deserialize_step_instructions_alias() {
    // The deserializer also accepts "step_instructions" as key
    let json = serde_json::json!({ "step_instructions": [["V"], ["count"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0], GremlinStep::V(None));
    assert_eq!(steps[1], GremlinStep::Count);
}

// ── json_to_gremlin_value coverage ──────────────────────────────────────

#[test]
fn json_to_gremlin_value_all_types() {
    assert_eq!(
        json_to_gremlin_value(&serde_json::json!(null)),
        GremlinValue::Null
    );
    assert_eq!(
        json_to_gremlin_value(&serde_json::json!(true)),
        GremlinValue::Bool(true)
    );
    assert_eq!(
        json_to_gremlin_value(&serde_json::json!(42)),
        GremlinValue::Integer(42)
    );
    assert_eq!(
        json_to_gremlin_value(&serde_json::json!("hi")),
        GremlinValue::String("hi".into())
    );
    assert_eq!(
        json_to_gremlin_value(&serde_json::json!([1, "a"])),
        GremlinValue::List(vec![
            GremlinValue::Integer(1),
            GremlinValue::String("a".into())
        ])
    );
    let map = json_to_gremlin_value(&serde_json::json!({"k": 99}));
    if let GremlinValue::Map(m) = &map {
        assert_eq!(m.get("k"), Some(&GremlinValue::Integer(99)));
    } else {
        panic!("expected Map, got {map:?}");
    }
}

#[test]
fn json_to_gremlin_value_float() {
    // f64 that isn't representable as i64
    let val = json_to_gremlin_value(&serde_json::json!(1.5));
    if let GremlinValue::Float(f) = val {
        assert!((f - 1.5).abs() < f64::EPSILON);
    } else {
        panic!("expected Float, got {val:?}");
    }
}

// ── Bytecode struct serde coverage ──────────────────────────────────────

#[test]
fn bytecode_struct_round_trip() {
    let bc = Bytecode {
        source_instructions: vec![("withSack".into(), vec![GremlinValue::Integer(0)])],
        step_instructions: vec![
            ("V".into(), vec![]),
            (
                "has".into(),
                vec![
                    GremlinValue::String("name".into()),
                    GremlinValue::String("marko".into()),
                ],
            ),
        ],
    };
    let json = serde_json::to_string(&bc).unwrap();
    let decoded: Bytecode = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, bc);
}

#[test]
fn bytecode_struct_defaults_empty() {
    let bc: Bytecode = serde_json::from_str("{}").unwrap();
    assert!(bc.source_instructions.is_empty());
    assert!(bc.step_instructions.is_empty());
}

// ── Error Display coverage ──────────────────────────────────────────────

#[test]
fn bytecode_error_display() {
    let e1 = BytecodeError::UnknownStep("foo".into());
    assert!(e1.to_string().contains("foo"));

    let e2 = BytecodeError::InvalidArgument {
        step: "has".into(),
        reason: "bad".into(),
    };
    assert!(e2.to_string().contains("has"));
    assert!(e2.to_string().contains("bad"));

    let e3 = BytecodeError::MissingArgument("limit".into());
    assert!(e3.to_string().contains("limit"));

    let e4 = BytecodeError::Json(serde_json::from_str::<i32>("bad").unwrap_err());
    assert!(!e4.to_string().is_empty());
}
