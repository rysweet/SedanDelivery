//! Tests for nested traversal parsing.
#![cfg(test)]

use super::*;

type SingleCtor = fn(Vec<GremlinStep>) -> GremlinStep;
type VariadicCtor = fn(Vec<Vec<GremlinStep>>) -> GremlinStep;

fn out_only() -> serde_json::Value {
    serde_json::json!({ "step": [["out"]] })
}

#[test]
fn deserialize_single_inner_steps() {
    // Each step that takes one nested traversal.
    let cases: &[(&str, SingleCtor)] = &[
        ("until", GremlinStep::Until),
        ("where", GremlinStep::Where),
        ("not", GremlinStep::Not),
        ("from", GremlinStep::From),
        ("to", GremlinStep::To),
        ("optional", GremlinStep::Optional),
        ("map", GremlinStep::Map),
        ("flatMap", GremlinStep::FlatMap),
        ("filter", GremlinStep::FilterTraversal),
        ("sideEffect", GremlinStep::SideEffect),
    ];
    for (name, ctor) in cases {
        let json = serde_json::json!({ "step": [[*name, out_only()]] });
        let steps = deserialize_bytecode(&json).unwrap_or_else(|e| panic!("{name} failed: {e}"));
        assert_eq!(steps[0], ctor(vec![GremlinStep::Out(vec![])]), "{name}");
    }
}

#[test]
fn deserialize_variadic_inner_steps() {
    let cases: &[(&str, VariadicCtor)] = &[
        ("and", GremlinStep::And),
        ("or", GremlinStep::Or),
        ("union", GremlinStep::Union),
        ("coalesce", GremlinStep::Coalesce),
        ("match", GremlinStep::Match),
    ];
    for (name, ctor) in cases {
        let json = serde_json::json!({
            "step": [[*name, out_only(), out_only()]]
        });
        let steps = deserialize_bytecode(&json).unwrap_or_else(|e| panic!("{name} failed: {e}"));
        assert_eq!(
            steps[0],
            ctor(vec![
                vec![GremlinStep::Out(vec![])],
                vec![GremlinStep::Out(vec![])]
            ]),
            "{name}"
        );
    }
}

#[test]
fn deserialize_choose_two_and_three_arg() {
    // Two-arg choose: (predicate, true_branch).
    let json = serde_json::json!({
        "step": [["choose", out_only(), out_only()]]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::Choose(
            vec![GremlinStep::Out(vec![])],
            vec![GremlinStep::Out(vec![])],
            None
        )
    );

    // Three-arg choose: (predicate, true_branch, false_branch).
    let json = serde_json::json!({
        "step": [["choose", out_only(), out_only(), out_only()]]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert!(matches!(steps[0], GremlinStep::Choose(_, _, Some(_))));
}

#[test]
fn deserialize_option_with_token_and_body() {
    let json = serde_json::json!({
        "step": [["option", "tokenA", out_only()]]
    });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::Option(
            GremlinValue::String("tokenA".into()),
            vec![GremlinStep::Out(vec![])]
        )
    );
}

#[test]
fn deserialize_by_string_and_traversal_forms() {
    let json = serde_json::json!({ "step": [["by", "name"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::By("name".into(), false));

    let json = serde_json::json!({ "step": [["by", "name", "desc"]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(steps[0], GremlinStep::By("name".into(), true));

    let json = serde_json::json!({ "step": [["by", out_only()]] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::ByTraversal(vec![GremlinStep::Out(vec![])], false)
    );
}

#[test]
fn deserialize_nested_inner_traversal_propagates_errors() {
    // Inner step is unknown — error should bubble with context.
    let json = serde_json::json!({
        "step": [["where", { "step": [["definitely_not_a_step"]] }]]
    });
    let err = deserialize_bytecode(&json).unwrap_err();
    match err {
        BytecodeError::InvalidArgument { step, reason } => {
            assert_eq!(step, "where");
            assert!(reason.contains("nested traversal"));
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}
