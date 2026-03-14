// range.rs — Range predicate evaluation helpers.

use gql_core::storage::RangeQuery;
use gql_core::types::Value;

// ── Range predicate evaluation ──────────────────────────────────────────────

pub(crate) fn range_matches(val: &Value, range: &RangeQuery<'_>) -> bool {
    match range {
        RangeQuery::Eq(target) => val.cypher_eq(target) == Some(true),
        RangeQuery::Lt(target) => cmp_values(val, target) == Some(std::cmp::Ordering::Less),
        RangeQuery::Lte(target) => {
            matches!(
                cmp_values(val, target),
                Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
            )
        }
        RangeQuery::Gt(target) => cmp_values(val, target) == Some(std::cmp::Ordering::Greater),
        RangeQuery::Gte(target) => {
            matches!(
                cmp_values(val, target),
                Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
            )
        }
        RangeQuery::Between { lo, hi, inclusive } => {
            let lo_ok = if *inclusive {
                matches!(
                    cmp_values(val, lo),
                    Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
                )
            } else {
                cmp_values(val, lo) == Some(std::cmp::Ordering::Greater)
            };
            let hi_ok = if *inclusive {
                matches!(
                    cmp_values(val, hi),
                    Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                )
            } else {
                cmp_values(val, hi) == Some(std::cmp::Ordering::Less)
            };
            lo_ok && hi_ok
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

pub(crate) fn cmp_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Some(x.cmp(y)),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y),
        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y),
        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)),
        (Value::Str(x), Value::Str(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    // ── cmp_values ──────────────────────────────────────────────────────────

    #[test]
    fn cmp_values_int_int() {
        assert_eq!(
            cmp_values(&Value::Int(1), &Value::Int(2)),
            Some(Ordering::Less)
        );
        assert_eq!(
            cmp_values(&Value::Int(2), &Value::Int(2)),
            Some(Ordering::Equal)
        );
        assert_eq!(
            cmp_values(&Value::Int(3), &Value::Int(2)),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn cmp_values_float_float() {
        assert_eq!(
            cmp_values(&Value::Float(1.0), &Value::Float(2.0)),
            Some(Ordering::Less)
        );
        assert_eq!(
            cmp_values(&Value::Float(2.0), &Value::Float(2.0)),
            Some(Ordering::Equal)
        );
        assert_eq!(
            cmp_values(&Value::Float(3.0), &Value::Float(2.0)),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn cmp_values_int_float_cross_type() {
        assert_eq!(
            cmp_values(&Value::Int(1), &Value::Float(2.0)),
            Some(Ordering::Less)
        );
        assert_eq!(
            cmp_values(&Value::Float(3.0), &Value::Int(2)),
            Some(Ordering::Greater)
        );
        assert_eq!(
            cmp_values(&Value::Int(2), &Value::Float(2.0)),
            Some(Ordering::Equal)
        );
    }

    #[test]
    fn cmp_values_string_string() {
        assert_eq!(
            cmp_values(&Value::Str("abc".into()), &Value::Str("def".into())),
            Some(Ordering::Less)
        );
        assert_eq!(
            cmp_values(&Value::Str("abc".into()), &Value::Str("abc".into())),
            Some(Ordering::Equal)
        );
    }

    #[test]
    fn cmp_values_incompatible_types_returns_none() {
        assert_eq!(
            cmp_values(&Value::Int(1), &Value::Str("hello".into())),
            None
        );
        assert_eq!(cmp_values(&Value::Bool(true), &Value::Int(1)), None);
        assert_eq!(cmp_values(&Value::Null, &Value::Int(1)), None);
    }

    #[test]
    fn cmp_values_float_nan_returns_none() {
        assert_eq!(
            cmp_values(&Value::Float(f64::NAN), &Value::Float(1.0)),
            None
        );
        assert_eq!(
            cmp_values(&Value::Float(1.0), &Value::Float(f64::NAN)),
            None
        );
    }

    // ── range_matches: Eq ───────────────────────────────────────────────────

    #[test]
    fn range_matches_eq_true() {
        let val = Value::Int(42);
        let target = Value::Int(42);
        assert!(range_matches(&val, &RangeQuery::Eq(&target)));
    }

    #[test]
    fn range_matches_eq_false() {
        let val = Value::Int(42);
        let target = Value::Int(99);
        assert!(!range_matches(&val, &RangeQuery::Eq(&target)));
    }

    #[test]
    fn range_matches_eq_cross_type_int_float() {
        let val = Value::Int(1);
        let target = Value::Float(1.0);
        // cypher_eq handles Int/Float coercion
        assert!(range_matches(&val, &RangeQuery::Eq(&target)));
    }

    #[test]
    fn range_matches_eq_null_propagation() {
        // NULL = anything is NULL (None), which does not equal Some(true)
        let val = Value::Null;
        let target = Value::Int(1);
        assert!(!range_matches(&val, &RangeQuery::Eq(&target)));
    }

    // ── range_matches: Lt ───────────────────────────────────────────────────

    #[test]
    fn range_matches_lt() {
        let target = Value::Int(10);
        assert!(range_matches(&Value::Int(5), &RangeQuery::Lt(&target)));
        assert!(!range_matches(&Value::Int(10), &RangeQuery::Lt(&target)));
        assert!(!range_matches(&Value::Int(15), &RangeQuery::Lt(&target)));
    }

    // ── range_matches: Lte ──────────────────────────────────────────────────

    #[test]
    fn range_matches_lte() {
        let target = Value::Int(10);
        assert!(range_matches(&Value::Int(5), &RangeQuery::Lte(&target)));
        assert!(range_matches(&Value::Int(10), &RangeQuery::Lte(&target)));
        assert!(!range_matches(&Value::Int(15), &RangeQuery::Lte(&target)));
    }

    // ── range_matches: Gt ───────────────────────────────────────────────────

    #[test]
    fn range_matches_gt() {
        let target = Value::Int(10);
        assert!(!range_matches(&Value::Int(5), &RangeQuery::Gt(&target)));
        assert!(!range_matches(&Value::Int(10), &RangeQuery::Gt(&target)));
        assert!(range_matches(&Value::Int(15), &RangeQuery::Gt(&target)));
    }

    // ── range_matches: Gte ──────────────────────────────────────────────────

    #[test]
    fn range_matches_gte() {
        let target = Value::Int(10);
        assert!(!range_matches(&Value::Int(5), &RangeQuery::Gte(&target)));
        assert!(range_matches(&Value::Int(10), &RangeQuery::Gte(&target)));
        assert!(range_matches(&Value::Int(15), &RangeQuery::Gte(&target)));
    }

    // ── range_matches: Between ──────────────────────────────────────────────

    #[test]
    fn range_matches_between_inclusive() {
        let lo = Value::Int(5);
        let hi = Value::Int(15);
        let range = RangeQuery::Between {
            lo: &lo,
            hi: &hi,
            inclusive: true,
        };
        assert!(range_matches(&Value::Int(5), &range)); // lo boundary
        assert!(range_matches(&Value::Int(10), &range)); // inside
        assert!(range_matches(&Value::Int(15), &range)); // hi boundary
        assert!(!range_matches(&Value::Int(4), &range)); // below
        assert!(!range_matches(&Value::Int(16), &range)); // above
    }

    #[test]
    fn range_matches_between_exclusive() {
        let lo = Value::Int(5);
        let hi = Value::Int(15);
        let range = RangeQuery::Between {
            lo: &lo,
            hi: &hi,
            inclusive: false,
        };
        assert!(!range_matches(&Value::Int(5), &range)); // lo boundary excluded
        assert!(range_matches(&Value::Int(10), &range)); // inside
        assert!(!range_matches(&Value::Int(15), &range)); // hi boundary excluded
    }

    #[test]
    fn range_matches_between_with_floats() {
        let lo = Value::Float(1.5);
        let hi = Value::Float(3.5);
        let range = RangeQuery::Between {
            lo: &lo,
            hi: &hi,
            inclusive: true,
        };
        assert!(range_matches(&Value::Float(2.0), &range));
        assert!(!range_matches(&Value::Float(0.5), &range));
        assert!(!range_matches(&Value::Float(4.0), &range));
    }

    // ── range_matches: StartsWith ───────────────────────────────────────────

    #[test]
    fn range_matches_starts_with_string() {
        assert!(range_matches(
            &Value::Str("hello world".into()),
            &RangeQuery::StartsWith("hello")
        ));
        assert!(!range_matches(
            &Value::Str("goodbye".into()),
            &RangeQuery::StartsWith("hello")
        ));
    }

    #[test]
    fn range_matches_starts_with_empty_prefix() {
        // Every string starts with the empty prefix.
        assert!(range_matches(
            &Value::Str("anything".into()),
            &RangeQuery::StartsWith("")
        ));
    }

    #[test]
    fn range_matches_starts_with_non_string_returns_false() {
        assert!(!range_matches(
            &Value::Int(42),
            &RangeQuery::StartsWith("4")
        ));
    }

    // ── range_matches: incompatible types ───────────────────────────────────

    #[test]
    fn range_matches_incompatible_types_return_false() {
        let target = Value::Str("hello".into());
        // Int vs String: cmp_values returns None, so Lt/Gt/etc. all false.
        assert!(!range_matches(&Value::Int(5), &RangeQuery::Lt(&target)));
        assert!(!range_matches(&Value::Int(5), &RangeQuery::Gt(&target)));
        assert!(!range_matches(&Value::Int(5), &RangeQuery::Lte(&target)));
        assert!(!range_matches(&Value::Int(5), &RangeQuery::Gte(&target)));
    }
}
