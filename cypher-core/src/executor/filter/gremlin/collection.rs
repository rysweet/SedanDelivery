//! Gremlin collection functions extracted from filter::eval_function_with_vals.
//!
//! Behavior preserved verbatim from the original `filter` module.

#![allow(clippy::needless_pass_by_value)]
#![allow(unused_imports)]

use indexmap::IndexMap;

use super::super::gremlin_pred::gremlin_pred_check;
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn combine(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        let lhs = vals.first().cloned().unwrap_or(Value::Null);
        let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
        match (&lhs, &rhs) {
                (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Incoming traverser for combine step can't be null".into(),
                    actual: "null".into(),
                })),
                (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for combine step must yield an iterable type, not null".into(),
                    actual: "null".into(),
                })),
                (Value::List(l), Value::List(r)) => {
                    let mut result = l.clone();
                    result.extend(r.iter().cloned());
                    Ok(Value::List(result))
                }
                (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "combine step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                    actual: format!("{:?}", lhs),
                })),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for combine step must yield an iterable type, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
            }
    }
}

pub(crate) fn merge(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        let lhs = vals.first().cloned().unwrap_or(Value::Null);
        let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
        match (&lhs, &rhs) {
                (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Incoming traverser for merge step can't be null".into(),
                    actual: "null".into(),
                })),
                (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for merge step must yield an iterable type, not null".into(),
                    actual: "null".into(),
                })),
                (Value::List(l), Value::List(r)) => {
                    // merge() is set-union: unique elements from both lists
                    let mut result = l.clone();
                    for item in r {
                        if !result.iter().any(|existing| existing == item) {
                            result.push(item.clone());
                        }
                    }
                    Ok(Value::List(result))
                }
                (Value::List(_), Value::Map(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "step type mismatch: expected argument to be Iterable but got Map".into(),
                    actual: "Map".into(),
                })),
                (Value::Map(l), Value::Map(r)) => {
                    // Map merge: right side wins on key collisions (TinkerPop semantics)
                    let mut result = l.clone();
                    for (k, v) in r.iter() {
                        result.insert(k.clone(), v.clone());
                    }
                    Ok(Value::Map(result))
                }
                (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "merge step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                    actual: format!("{:?}", lhs),
                })),
                (_, Value::Map(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "merge step expected provided argument to evaluate to a Map, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for merge step must yield an iterable type, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
            }
    }
}

pub(crate) fn intersect(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        let lhs = vals.first().cloned().unwrap_or(Value::Null);
        let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
        match (&lhs, &rhs) {
                (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Incoming traverser for intersect step can't be null".into(),
                    actual: "null".into(),
                })),
                (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for intersect step must yield an iterable type, not null".into(),
                    actual: "null".into(),
                })),
                (Value::List(l), Value::List(r)) => {
                    // Multiset intersection: use swap_remove for O(1) removal
                    // instead of remove() which is O(N) due to element shifting.
                    let mut result = Vec::new();
                    let mut rhs_remaining: Vec<&Value> = r.iter().collect();
                    for item in l {
                        if let Some(pos) = rhs_remaining.iter().position(|rv| *rv == item) {
                            result.push(item.clone());
                            rhs_remaining.swap_remove(pos);
                        }
                    }
                    Ok(Value::List(result))
                }
                (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "intersect step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                    actual: format!("{:?}", lhs),
                })),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for intersect step must yield an iterable type, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
            }
    }
}

pub(crate) fn difference(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        let lhs = vals.first().cloned().unwrap_or(Value::Null);
        let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
        match (&lhs, &rhs) {
                (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Incoming traverser for difference step can't be null".into(),
                    actual: "null".into(),
                })),
                (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for difference step must yield an iterable type, not null".into(),
                    actual: "null".into(),
                })),
                (Value::List(l), Value::List(r)) => {
                    // Multiset difference: use swap_remove for O(1) removal
                    let mut result = Vec::new();
                    let mut rhs_remaining: Vec<&Value> = r.iter().collect();
                    for item in l {
                        if let Some(pos) = rhs_remaining.iter().position(|rv| *rv == item) {
                            rhs_remaining.swap_remove(pos);
                        } else {
                            result.push(item.clone());
                        }
                    }
                    Ok(Value::List(result))
                }
                (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "difference step can only take an array or an Iterable type for incoming traversers".into(),
                    actual: format!("{:?}", lhs),
                })),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for difference step must yield an iterable type, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
            }
    }
}

pub(crate) fn disjunct(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        let lhs = vals.first().cloned().unwrap_or(Value::Null);
        let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
        match (&lhs, &rhs) {
                (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Incoming traverser for disjunct step can't be null".into(),
                    actual: "null".into(),
                })),
                (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for disjunct step must yield an iterable type, not null".into(),
                    actual: "null".into(),
                })),
                (Value::List(l), Value::List(r)) => {
                    // Symmetric difference: use swap_remove for O(1) removal
                    let mut result = Vec::new();
                    let mut r_remaining: Vec<&Value> = r.iter().collect();
                    for item in l {
                        if let Some(pos) = r_remaining.iter().position(|rv| *rv == item) {
                            r_remaining.swap_remove(pos);
                        } else {
                            result.push(item.clone());
                        }
                    }
                    for item in r_remaining {
                        result.push(item.clone());
                    }
                    Ok(Value::List(result))
                }
                (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "disjunct step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                    actual: format!("{:?}", lhs),
                })),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for disjunct step must yield an iterable type, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
            }
    }
}

pub(crate) fn product(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        let lhs = vals.first().cloned().unwrap_or(Value::Null);
        let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
        match (&lhs, &rhs) {
                (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Incoming traverser for product step can't be null".into(),
                    actual: "null".into(),
                })),
                (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for product step must yield an iterable type, not null".into(),
                    actual: "null".into(),
                })),
                (Value::List(l), Value::List(r)) => {
                    if l.is_empty() || r.is_empty() {
                        return Ok(Value::List(Vec::new()));
                    }
                    let mut result = Vec::new();
                    for a in l {
                        for b in r {
                            result.push(Value::List(vec![a.clone(), b.clone()]));
                        }
                    }
                    Ok(Value::List(result))
                }
                (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "product step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                    actual: format!("{:?}", lhs),
                })),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for product step must yield an iterable type, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
            }
    }
}

pub(crate) fn all(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // all(pred): filter step — returns the list if ALL elements match, else filters out
        let list_val = vals.first().cloned().unwrap_or(Value::Null);
        match &list_val {
            Value::List(items) => {
                // pred info in vals[1] (op string) and vals[2] (value)
                let pred_op = match vals.get(1) {
                    Some(Value::Str(s)) => s.as_ref(),
                    _ => "neq",
                };
                let pred_val = vals.get(2).cloned().unwrap_or(Value::Null);
                let all_match = items
                    .iter()
                    .all(|v| gremlin_pred_check(v, pred_op, &pred_val));
                Ok(Value::Bool(all_match))
            }
            _ => Ok(Value::Bool(false)),
        }
    }
}

pub(crate) fn any(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        let list_val = vals.first().cloned().unwrap_or(Value::Null);
        match &list_val {
            Value::List(items) => {
                let pred_op = match vals.get(1) {
                    Some(Value::Str(s)) => s.as_ref(),
                    _ => "neq",
                };
                let pred_val = vals.get(2).cloned().unwrap_or(Value::Null);
                let any_match = items
                    .iter()
                    .any(|v| gremlin_pred_check(v, pred_op, &pred_val));
                Ok(Value::Bool(any_match))
            }
            _ => Ok(Value::Bool(false)),
        }
    }
}

pub(crate) fn none(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        let list_val = vals.first().cloned().unwrap_or(Value::Null);
        let pred_op = match vals.get(1) {
            Some(Value::Str(s)) => s.as_ref(),
            _ => "neq",
        };
        let pred_val = vals.get(2).cloned().unwrap_or(Value::Null);
        match &list_val {
            Value::List(items) => {
                let none_match = !items
                    .iter()
                    .any(|v| gremlin_pred_check(v, pred_op, &pred_val));
                Ok(Value::Bool(none_match))
            }
            // Scalar: none(P) is a collection predicate — always false
            // for non-collection values.
            _ => Ok(Value::Bool(false)),
        }
    }
}

pub(crate) fn wraplist(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // Wrap a single value in a list for valueMap
        match vals.into_iter().next() {
            Some(v) => Ok(Value::List(vec![v])),
            None => Ok(Value::List(Vec::new())),
        }
    }
}

pub(crate) fn valuemap(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // Full valueMap: extract all properties and wrap in lists.
        // Multi-valued properties (stored as List of Maps with "value" key)
        // are flattened to a list of the "value" fields, matching TinkerPop
        // multi-property semantics.
        match vals.into_iter().next() {
            Some(Value::Map(m)) => {
                let wrapped: IndexMap<Box<str>, Value> = m
                    .into_iter()
                    .map(|(k, v)| {
                        // Detect multi-property: List of Maps with "value" key
                        if let Value::List(ref items) = v {
                            let all_maps_with_value = !items.is_empty()
                                && items.iter().all(
                                    |item| matches!(item, Value::Map(m) if m.contains_key("value")),
                                );
                            if all_maps_with_value {
                                let extracted: Vec<Value> = items
                                    .iter()
                                    .filter_map(|item| match item {
                                        Value::Map(m) => m.get("value").cloned(),
                                        _ => None,
                                    })
                                    .collect();
                                return (k, Value::List(extracted));
                            }
                        }
                        (k, Value::List(vec![v]))
                    })
                    .collect();
                Ok(Value::Map(wrapped))
            }
            Some(other) => Ok(other),
            None => Ok(Value::Null),
        }
    }
}

pub(crate) fn valuemap_unfold(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // Unwrap list values in a valueMap result (by(__.unfold()))
        match vals.into_iter().next() {
            Some(Value::Map(m)) => {
                let unwrapped: IndexMap<Box<str>, Value> = m
                    .into_iter()
                    .map(|(k, v)| {
                        let unwrapped = match v {
                            Value::List(items) if items.len() == 1 => {
                                items.into_iter().next().unwrap_or(Value::Null)
                            }
                            other => other,
                        };
                        (k, unwrapped)
                    })
                    .collect();
                Ok(Value::Map(unwrapped))
            }
            Some(other) => Ok(other),
            None => Ok(Value::Null),
        }
    }
}

pub(crate) fn values_all(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // Fallback for non-graph values (maps, etc.)
        match vals.into_iter().next() {
            Some(Value::Map(m)) => {
                // Property Map from properties(): extract ~value
                if m.contains_key("~value") {
                    let val = m.get("~value").cloned().unwrap_or(Value::Null);
                    Ok(Value::List(vec![val]))
                } else {
                    let vals: Vec<Value> = m.into_values().collect();
                    Ok(Value::List(vals))
                }
            }
            Some(Value::Null) | None => Ok(Value::List(vec![])),
            Some(other) => Ok(Value::List(vec![other])),
        }
    }
}
