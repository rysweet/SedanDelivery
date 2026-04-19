use super::distinct::DistinctSet;
use super::group_key::{group_key_hash, group_key_string};
use super::state::AggState;

use crate::ast::{Expr, Identifier, Label};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::QueryExecutor;
use crate::planner::{AggFunction, Operator};
use crate::storage::{GraphStore, InMemoryGraphStore};
use crate::types::Value;
use indexmap::IndexMap;

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

#[test]
fn agg_count() {
    let mut s = AggState::default();
    s.accumulate(&AggFunction::Count, Value::Int(1), None, false);
    s.accumulate(&AggFunction::Count, Value::Null, None, false);
    assert_eq!(s.finalize(&AggFunction::Count).unwrap(), Value::Int(1));
}

#[test]
fn agg_sum() {
    let mut s = AggState::default();
    s.accumulate(&AggFunction::Sum, Value::Int(10), None, false);
    s.accumulate(&AggFunction::Sum, Value::Int(20), None, false);
    assert_eq!(s.finalize(&AggFunction::Sum).unwrap(), Value::Float(30.0));
}

#[test]
fn agg_avg() {
    let mut s = AggState::default();
    s.accumulate(&AggFunction::Avg, Value::Int(10), None, false);
    s.accumulate(&AggFunction::Avg, Value::Int(20), None, false);
    assert_eq!(s.finalize(&AggFunction::Avg).unwrap(), Value::Float(15.0));
}

#[test]
fn agg_min_max() {
    let mut min_s = AggState::default();
    min_s.accumulate(&AggFunction::Min, Value::Int(5), None, false);
    min_s.accumulate(&AggFunction::Min, Value::Int(2), None, false);
    assert_eq!(min_s.finalize(&AggFunction::Min).unwrap(), Value::Int(2));

    let mut max_s = AggState::default();
    max_s.accumulate(&AggFunction::Max, Value::Int(5), None, false);
    max_s.accumulate(&AggFunction::Max, Value::Int(8), None, false);
    assert_eq!(max_s.finalize(&AggFunction::Max).unwrap(), Value::Int(8));
}

#[test]
fn agg_collect() {
    let mut s = AggState::default();
    s.accumulate(&AggFunction::Collect, Value::Int(1), None, false);
    s.accumulate(&AggFunction::Collect, Value::Int(2), None, false);
    s.accumulate(&AggFunction::Collect, Value::Null, None, false);
    assert_eq!(
        s.finalize(&AggFunction::Collect).unwrap(),
        Value::List(vec![Value::Int(1), Value::Int(2)])
    );
}

#[test]
fn agg_distinct() {
    let mut s = AggState::default();
    s.accumulate(&AggFunction::Count, Value::Int(1), None, true);
    s.accumulate(&AggFunction::Count, Value::Int(1), None, true);
    s.accumulate(&AggFunction::Count, Value::Int(2), None, true);
    assert_eq!(s.finalize(&AggFunction::Count).unwrap(), Value::Int(2));
}

#[test]
fn distinct_set_basic() {
    let mut ds = DistinctSet::new();
    assert!(ds.insert(&Value::Int(1)));
    assert!(!ds.insert(&Value::Int(1))); // duplicate
    assert!(ds.insert(&Value::Int(2)));
    assert_eq!(ds.len(), 2);
}

#[test]
fn distinct_set_float_neg_zero() {
    let mut ds = DistinctSet::new();
    assert!(ds.insert(&Value::Float(0.0)));
    assert!(!ds.insert(&Value::Float(-0.0))); // -0.0 == 0.0
    assert_eq!(ds.len(), 1);
}

#[test]
fn distinct_set_mixed_types() {
    let mut ds = DistinctSet::new();
    assert!(ds.insert(&Value::Int(1)));
    assert!(ds.insert(&Value::Str("1".into())));
    assert!(ds.insert(&Value::Float(1.0)));
    assert_eq!(ds.len(), 3);
}

#[test]
fn group_key_hash_deterministic() {
    let key1 = vec![Value::Int(1), Value::Str("a".into())];
    let key2 = vec![Value::Int(1), Value::Str("a".into())];
    assert_eq!(group_key_hash(&key1), group_key_hash(&key2));
}

#[test]
fn group_key_hash_different_keys_differ() {
    let key1 = vec![Value::Int(1)];
    let key2 = vec![Value::Int(2)];
    assert_ne!(group_key_hash(&key1), group_key_hash(&key2));
}

#[test]
fn group_key_string_deterministic() {
    let key = vec![Value::Int(1), Value::Str("hello".into())];
    let s1 = group_key_string(&key);
    let s2 = group_key_string(&key);
    assert_eq!(s1, s2);
}

#[test]
fn exec_aggregate_count() {
    let store = make_store();
    for _ in 0..3 {
        store
            .create_node("g", vec!["P".to_string()], IndexMap::new())
            .unwrap();
    }
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Aggregate {
        keys: vec![],
        aggregations: vec![(
            AggFunction::Count,
            Expr::Variable(Identifier::new("n")),
            None,
            false,
            Identifier::new("c"),
        )],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows[0]["c"], Value::Int(3));
}

#[test]
fn exec_aggregate_empty_input() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Aggregate {
        keys: vec![],
        aggregations: vec![(
            AggFunction::Count,
            Expr::Variable(Identifier::new("*")),
            None,
            false,
            Identifier::new("c"),
        )],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("X")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows[0]["c"], Value::Int(0));
}
