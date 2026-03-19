//! Value comparison and ordering for ORDER BY / min / max.

use crate::executor::filter::time_to_utc_nanos;
use crate::types::Value;

/// Convert a civil time to nanoseconds-from-midnight.
pub(crate) fn civil_time_to_nanos(t: &jiff::civil::Time) -> i64 {
    t.hour() as i64 * 3_600_000_000_000
        + t.minute() as i64 * 60_000_000_000
        + t.second() as i64 * 1_000_000_000
        + t.subsec_nanosecond() as i64
}

/// Compare two `Value`s for ordering purposes.
///
/// Ordering follows openCypher/Neo4j semantics for ORDER BY / min / max.
/// Ascending order (from smallest to largest):
///   Node/Edge/Path < Map < List < Bool < Str < Number < Temporal < Null
/// Null sorts LAST in ascending order (treated as the "greatest" value).
pub fn cmp_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;

    // Type ordering (ascending, smallest to largest) per openCypher spec:
    // Map < Node < Edge < List < Path < String < Bool < Number < NaN < Null
    // Temporal types (Date, Time, DateTime, Duration) sit between Bool and Null.
    fn type_rank(v: &Value) -> u8 {
        match v {
            Value::Map(_) => 0,
            Value::Node(_) => 1,
            Value::Edge(_) => 2,
            Value::List(_) => 3,
            Value::Path { .. } => 4,
            Value::Str(_) => 5,
            Value::Bool(_) => 6,
            Value::Float(f) if f.is_nan() => 8, // NaN sorts after regular numbers
            Value::Int(_) | Value::Float(_) => 7,
            Value::Date(_) => 10,
            Value::LocalTime(_) => 11,
            Value::Time(_, _) => 11, // Zoned time sorts same rank as LocalTime
            Value::LocalDateTime(_) => 12,
            Value::DateTime(_) => 13,
            Value::Duration(_) => 14,
            Value::Point(_) => 15,
            Value::Bytes(_) => 16,
            Value::Null => 255, // Null sorts LAST (largest) in ascending order
        }
    }

    let ra = type_rank(a);
    let rb = type_rank(b);
    if ra != rb {
        return ra.cmp(&rb);
    }

    match (a, b) {
        (Value::Null, Value::Null) => Equal,
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Float(x), Value::Float(y)) => {
            // Both are either NaN (rank 8) or regular floats (rank 7).
            // If same rank, they're the same category; NaN == NaN, regular == regular.
            x.partial_cmp(y).unwrap_or(Equal)
        }
        (Value::Int(x), Value::Float(y)) => {
            crate::executor::filter::operators::safe_int_float_cmp(*x, *y).unwrap_or(Equal)
        }
        (Value::Float(x), Value::Int(y)) => {
            crate::executor::filter::operators::safe_int_float_cmp(*y, *x)
                .map(|o| o.reverse())
                .unwrap_or(Equal)
        }
        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        (Value::Date(x), Value::Date(y)) => x.cmp(y),
        (Value::LocalTime(x), Value::LocalTime(y)) => x.cmp(y),
        // Zoned time: compare by UTC-normalized instant
        (Value::Time(t1, o1), Value::Time(t2, o2)) => {
            let utc1 = time_to_utc_nanos(t1, *o1);
            let utc2 = time_to_utc_nanos(t2, *o2);
            utc1.cmp(&utc2)
        }
        // Mixed LocalTime / Time: LocalTime treated as-is, Time UTC-normalized
        (Value::LocalTime(t1), Value::Time(t2, o2)) => {
            let n1 = civil_time_to_nanos(t1);
            let n2 = time_to_utc_nanos(t2, *o2);
            n1.cmp(&n2)
        }
        (Value::Time(t1, o1), Value::LocalTime(t2)) => {
            let n1 = time_to_utc_nanos(t1, *o1);
            let n2 = civil_time_to_nanos(t2);
            n1.cmp(&n2)
        }
        (Value::LocalDateTime(x), Value::LocalDateTime(y)) => x.cmp(y),
        (Value::DateTime(x), Value::DateTime(y)) => x.cmp(y),
        (Value::List(x), Value::List(y)) => {
            for (a, b) in x.iter().zip(y.iter()) {
                let ord = cmp_values(a, b);
                if ord != Equal {
                    return ord;
                }
            }
            x.len().cmp(&y.len())
        }
        // Same-rank types without a natural total order — use deterministic
        // ordering so that ORDER BY never silently treats distinct values as equal.
        (Value::Map(x), Value::Map(y)) => x.len().cmp(&y.len()),
        (Value::Node(x), Value::Node(y)) => x.cmp(y),
        (Value::Edge(x), Value::Edge(y)) => x.cmp(y),
        (
            Value::Path {
                nodes: n1,
                edges: e1,
            },
            Value::Path {
                nodes: n2,
                edges: e2,
            },
        ) => n1
            .len()
            .cmp(&n2.len())
            .then_with(|| n1.cmp(n2))
            .then_with(|| e1.cmp(e2)),
        (Value::Duration(d1), Value::Duration(d2)) => d1
            .months
            .cmp(&d2.months)
            .then_with(|| d1.days.cmp(&d2.days))
            .then_with(|| d1.nanos.cmp(&d2.nanos)),
        (Value::Point(p1), Value::Point(p2)) => {
            p1.x.total_cmp(&p2.x)
                .then_with(|| p1.y.total_cmp(&p2.y))
                .then_with(|| match (p1.z, p2.z) {
                    (Some(a), Some(b)) => a.total_cmp(&b),
                    (None, Some(_)) => Less,
                    (Some(_), None) => Greater,
                    (None, None) => Equal,
                })
        }
        (Value::Bytes(x), Value::Bytes(y)) => x.cmp(y),
        // Truly unreachable: every variant pair within the same type_rank is
        // handled above.  Fall back to Equal to keep the compiler happy.
        _ => Equal,
    }
}

/// Compare two `Value`s using Gremlin/TinkerPop orderability semantics.
///
/// Gremlin ascending order: Null < Bool < Number < Date < String < UUID
///   < Vertex < Edge < Property < Path < Set < List < Map
pub fn gremlin_cmp_values(a: &Value, b: &Value) -> std::cmp::Ordering {

    fn type_rank(v: &Value) -> u8 {
        match v {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Float(f) if f.is_nan() => 3,
            Value::Int(_) | Value::Float(_) => 2,
            Value::Date(_) => 4,
            Value::LocalTime(_) | Value::Time(_, _) => 5,
            Value::LocalDateTime(_) => 6,
            Value::DateTime(_) => 7,
            Value::Duration(_) => 8,
            Value::Str(_) => 10,
            Value::Bytes(_) => 11,
            Value::Node(_) => 20,
            Value::Edge(_) => 21,
            Value::Path { .. } => 22,
            Value::List(_) => 30,
            Value::Map(_) => 31,
            Value::Point(_) => 32,
        }
    }

    let ra = type_rank(a);
    let rb = type_rank(b);
    if ra != rb {
        return ra.cmp(&rb);
    }

    // Delegate same-type comparison to the existing logic.
    cmp_values(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn cmp_values_null_last() {
        assert_eq!(cmp_values(&Value::Null, &Value::Int(0)), Ordering::Greater);
        assert_eq!(cmp_values(&Value::Null, &Value::Null), Ordering::Equal);
    }

    #[test]
    fn cmp_values_nan_after_regular() {
        assert_eq!(
            cmp_values(&Value::Float(f64::NAN), &Value::Float(1.0)),
            Ordering::Greater
        );
    }

    #[test]
    fn cmp_values_list_lex() {
        let a = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let b = Value::List(vec![Value::Int(1), Value::Int(3)]);
        assert_eq!(cmp_values(&a, &b), Ordering::Less);
    }
}
