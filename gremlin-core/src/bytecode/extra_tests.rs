//! Additional unit tests for bytecode submodule edge cases (display, conversion, serde).
#![cfg(test)]

use super::*;
use crate::types::GremlinValue;

#[test]
fn pop_serde_round_trip_all_variants() {
    for pop in [Pop::First, Pop::Last, Pop::All, Pop::Mixed] {
        let s = serde_json::to_string(&pop).unwrap();
        let back: Pop = serde_json::from_str(&s).unwrap();
        assert_eq!(pop, back);
    }
}

#[test]
fn predicate_serde_round_trip_simple() {
    let p = Predicate::Eq(GremlinValue::Integer(42));
    let s = serde_json::to_string(&p).unwrap();
    let back: Predicate = serde_json::from_str(&s).unwrap();
    assert_eq!(p, back);
}

#[test]
fn predicate_serde_round_trip_nested_and_or_not() {
    let p = Predicate::And(vec![
        Predicate::Or(vec![
            Predicate::Gt(GremlinValue::Integer(1)),
            Predicate::Lt(GremlinValue::Integer(10)),
        ]),
        Predicate::Not(Box::new(Predicate::Eq(GremlinValue::Integer(5)))),
    ]);
    let s = serde_json::to_string(&p).unwrap();
    let back: Predicate = serde_json::from_str(&s).unwrap();
    assert_eq!(p, back);
}

#[test]
fn predicate_text_variants_round_trip() {
    let preds = [
        Predicate::Containing(GremlinValue::String("ab".into())),
        Predicate::NotContaining(GremlinValue::String("ab".into())),
        Predicate::StartingWith(GremlinValue::String("a".into())),
        Predicate::EndingWith(GremlinValue::String("z".into())),
        Predicate::Regex(GremlinValue::String("^x$".into())),
    ];
    for p in preds {
        let s = serde_json::to_string(&p).unwrap();
        let back: Predicate = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }
}

#[test]
fn predicate_within_without_round_trip() {
    let p = Predicate::Within(vec![
        GremlinValue::Integer(1),
        GremlinValue::Integer(2),
        GremlinValue::Integer(3),
    ]);
    let s = serde_json::to_string(&p).unwrap();
    assert_eq!(p, serde_json::from_str::<Predicate>(&s).unwrap());

    let p2 = Predicate::Without(vec![GremlinValue::String("x".into())]);
    let s2 = serde_json::to_string(&p2).unwrap();
    assert_eq!(p2, serde_json::from_str::<Predicate>(&s2).unwrap());
}

#[test]
fn predicate_between_inside_round_trip() {
    let p = Predicate::Between(GremlinValue::Integer(1), GremlinValue::Integer(10));
    let s = serde_json::to_string(&p).unwrap();
    assert_eq!(p, serde_json::from_str::<Predicate>(&s).unwrap());

    let p2 = Predicate::Inside(GremlinValue::Integer(1), GremlinValue::Integer(10));
    let s2 = serde_json::to_string(&p2).unwrap();
    assert_eq!(p2, serde_json::from_str::<Predicate>(&s2).unwrap());
}

#[test]
fn bytecode_struct_default_via_serde() {
    let bc: Bytecode = serde_json::from_str("{}").unwrap();
    assert!(bc.source_instructions.is_empty());
    assert!(bc.step_instructions.is_empty());
}

#[test]
fn bytecode_struct_round_trip() {
    let bc = Bytecode {
        source_instructions: vec![(
            "withStrategies".into(),
            vec![GremlinValue::String("X".into())],
        )],
        step_instructions: vec![("V".into(), vec![])],
    };
    let s = serde_json::to_string(&bc).unwrap();
    let back: Bytecode = serde_json::from_str(&s).unwrap();
    assert_eq!(bc, back);
}

#[test]
fn bytecode_error_display_variants() {
    let e1 = BytecodeError::UnknownStep("foo".into());
    assert!(e1.to_string().contains("foo"));
    let e2 = BytecodeError::InvalidArgument {
        step: "has".into(),
        reason: "nope".into(),
    };
    let s = e2.to_string();
    assert!(s.contains("has") && s.contains("nope"));
    let e3 = BytecodeError::MissingArgument("V".into());
    assert!(e3.to_string().contains("V"));
}

#[test]
fn deserialize_empty_step_array() {
    let json = serde_json::json!({ "step": [] });
    let steps = deserialize_bytecode(&json).unwrap();
    assert!(steps.is_empty());
}

#[test]
fn deserialize_missing_step_key_errors() {
    let json = serde_json::json!({});
    let err = deserialize_bytecode(&json).unwrap_err();
    assert!(matches!(err, BytecodeError::InvalidArgument { .. }));
}

#[test]
fn gremlin_step_clone_eq_for_v_none() {
    let a = GremlinStep::V(None);
    let b = a.clone();
    assert_eq!(a, b);
}
