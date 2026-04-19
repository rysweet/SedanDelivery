//! Gremlin predicate helpers for `all()`/`any()`/`none()` and sort-key extraction.
//!
//! Behavior preserved verbatim from the original `filter` module.

use crate::types::Value;

// ── Gremlin predicate helpers ─────────────────────────────────────────────────

/// Evaluate a Gremlin predicate check for all()/any()/none() steps.
pub(crate) fn gremlin_pred_check(val: &Value, pred_op: &str, pred_val: &Value) -> bool {
    match pred_op {
        "eq" => gremlin_eq(val, pred_val),
        "neq" => !gremlin_eq(val, pred_val),
        "gt" => gremlin_cmp(val, pred_val) == Some(std::cmp::Ordering::Greater),
        "gte" => matches!(
            gremlin_cmp(val, pred_val),
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ),
        "lt" => gremlin_cmp(val, pred_val) == Some(std::cmp::Ordering::Less),
        "lte" => matches!(
            gremlin_cmp(val, pred_val),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ),
        "within" => {
            if let Value::List(options) = pred_val {
                options.iter().any(|o| gremlin_eq(val, o))
            } else {
                false
            }
        }
        "without" => {
            if let Value::List(options) = pred_val {
                !options.iter().any(|o| gremlin_eq(val, o))
            } else {
                true
            }
        }
        _ => false,
    }
}

/// Gremlin equality: null == null is true, NaN != NaN, Int/Float coercion.
pub(crate) fn gremlin_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::Float(f), _) if f.is_nan() => false,
        (_, Value::Float(f)) if f.is_nan() => false,
        (Value::Int(i), Value::Float(f)) | (Value::Float(f), Value::Int(i)) => {
            let fi = *i as f64;
            fi as i64 == *i && fi == *f
        }
        _ => a == b,
    }
}

/// Gremlin ordering for comparable values.
pub(crate) fn gremlin_cmp(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Null, Value::Null) => Some(std::cmp::Ordering::Equal),
        (Value::Null, _) | (_, Value::Null) => None,
        (Value::Float(f), _) if f.is_nan() => None,
        (_, Value::Float(f)) if f.is_nan() => None,
        (Value::Int(i1), Value::Int(i2)) => i1.partial_cmp(i2),
        (Value::Float(f1), Value::Float(f2)) => f1.partial_cmp(f2),
        (Value::Int(i), Value::Float(f)) => (*i as f64).partial_cmp(f),
        (Value::Float(f), Value::Int(i)) => f.partial_cmp(&(*i as f64)),
        (Value::Str(s1), Value::Str(s2)) => Some(s1.cmp(s2)),
        (Value::Bool(b1), Value::Bool(b2)) => b1.partial_cmp(b2),
        _ => None,
    }
}

/// Extract a sort key from a value (node, edge, or map) by property name.
pub(crate) fn extract_sort_key(v: &Value, key: &str) -> Value {
    match v {
        Value::Map(m) => m.get(key).cloned().unwrap_or(Value::Null),
        // For nodes/edges, we can't directly access properties here
        // since we don't have GraphStore access in filter context.
        _ => Value::Null,
    }
}
