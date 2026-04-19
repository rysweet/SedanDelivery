//! Simple range matching (test-only — used to validate range/index helpers).
//!
//! Duplicated from sd2-redis-store/src/range.rs since we cannot depend on that
//! crate without pulling in the entire Redis TCP client.

#![cfg(test)]

use cypher_core::storage::RangeQuery;
use cypher_core::types::Value;

pub(super) fn range_matches(val: &Value, range: &RangeQuery<'_>) -> bool {
    match range {
        RangeQuery::Eq(expected) => val == *expected,
        RangeQuery::Lt(bound) => partial_cmp_values(val, bound).is_some_and(|o| o.is_lt()),
        RangeQuery::Lte(bound) => partial_cmp_values(val, bound).is_some_and(|o| !o.is_gt()),
        RangeQuery::Gt(bound) => partial_cmp_values(val, bound).is_some_and(|o| o.is_gt()),
        RangeQuery::Gte(bound) => partial_cmp_values(val, bound).is_some_and(|o| !o.is_lt()),
        RangeQuery::Between { lo, hi, inclusive } => {
            let ge_lo = partial_cmp_values(val, lo).is_some_and(|o| !o.is_lt());
            let le_hi = if *inclusive {
                partial_cmp_values(val, hi).is_some_and(|o| !o.is_gt())
            } else {
                partial_cmp_values(val, hi).is_some_and(|o| o.is_lt())
            };
            ge_lo && le_hi
        }
        RangeQuery::StartsWith(prefix) => {
            if let Value::Str(s) = val {
                s.starts_with(prefix)
            } else {
                false
            }
        }
    }
}

fn partial_cmp_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => a.partial_cmp(b),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
        (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
        (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
        (Value::Str(a), Value::Str(b)) => a.partial_cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.partial_cmp(b),
        _ => None,
    }
}
