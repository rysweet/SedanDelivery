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
    /// Detect a property Map: a Map with keys `~key`, `~value`, `~element`.
    fn is_property_map(v: &Value) -> bool {
        if let Value::Map(m) = v {
            m.contains_key("~key") && m.contains_key("~value") && m.contains_key("~element")
        } else {
            false
        }
    }

    fn is_tagged_set(v: &Value) -> bool {
        matches!(v, Value::Map(m) if m.len() == 1 && matches!(m.get("~set"), Some(Value::List(_))))
    }

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
            // UUID strings (tagged with ~uuid: prefix) rank between String and Vertex
            Value::Str(s) if s.starts_with("~uuid:") => 12,
            Value::Str(_) => 10,
            Value::Bytes(_) => 11,
            Value::Node(_) => 20,
            Value::Edge(_) => 21,
            // Property maps rank between Edge and Path in Gremlin orderability.
            Value::Map(m)
                if m.contains_key("~key")
                    && m.contains_key("~value")
                    && m.contains_key("~element") =>
            {
                22
            }
            Value::Path { .. } => 24,
            Value::Map(m) if m.len() == 1 && matches!(m.get("~set"), Some(Value::List(_))) => 29,
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

    // Property maps: comparison depends on element type.
    // Vertex properties (VertexProperty extends Element): by element, then ID.
    // Edge properties (Property): by key, then value.
    if is_property_map(a) && is_property_map(b) {
        if let (Value::Map(ma), Value::Map(mb)) = (a, b) {
            let ea = ma.get("~element").unwrap_or(&Value::Null);
            let eb = mb.get("~element").unwrap_or(&Value::Null);

            if matches!(ea, Value::Node(_)) && matches!(eb, Value::Node(_)) {
                // VertexProperty: order by owning vertex, then property ID.
                let elem_cmp = gremlin_cmp_values(ea, eb);
                if elem_cmp != std::cmp::Ordering::Equal {
                    return elem_cmp;
                }
                let ida = ma.get("~id").unwrap_or(&Value::Null);
                let idb = mb.get("~id").unwrap_or(&Value::Null);
                return gremlin_cmp_values(ida, idb);
            }

            // Edge Property: order by key name, then value.
            let ka = ma.get("~key").unwrap_or(&Value::Null);
            let kb = mb.get("~key").unwrap_or(&Value::Null);
            let key_cmp = gremlin_cmp_values(ka, kb);
            if key_cmp != std::cmp::Ordering::Equal {
                return key_cmp;
            }
            let va = ma.get("~value").unwrap_or(&Value::Null);
            let vb = mb.get("~value").unwrap_or(&Value::Null);
            return gremlin_cmp_values(va, vb);
        }
    }

    if is_tagged_set(a) && is_tagged_set(b) {
        if let (Value::Map(ma), Value::Map(mb)) = (a, b) {
            let sa = ma.get("~set").unwrap_or(&Value::Null);
            let sb = mb.get("~set").unwrap_or(&Value::Null);
            return gremlin_cmp_values(sa, sb);
        }
    }

    if let (Value::Map(ma), Value::Map(mb)) = (a, b) {
        for ((ka, va), (kb, vb)) in ma.iter().zip(mb.iter()) {
            let key_cmp = ka.cmp(kb);
            if key_cmp != std::cmp::Ordering::Equal {
                return key_cmp;
            }
            let value_cmp = gremlin_cmp_values(va, vb);
            if value_cmp != std::cmp::Ordering::Equal {
                return value_cmp;
            }
        }
        return ma.len().cmp(&mb.len());
    }

    // Lists: use gremlin_cmp_values recursively so that nested property
    // maps, vertices, etc. follow Gremlin orderability within list elements.
    if let (Value::List(x), Value::List(y)) = (a, b) {
        for (ea, eb) in x.iter().zip(y.iter()) {
            let ord = gremlin_cmp_values(ea, eb);
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        return x.len().cmp(&y.len());
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

    #[test]
    fn cmp_values_type_rank_orders_categories() {
        // Map < Node < Edge < List < Path < String < Bool < Number < Null
        use crate::storage::{EdgeId, NodeId};
        let map = Value::Map(indexmap::IndexMap::new());
        let node = Value::Node(NodeId(0));
        let edge = Value::Edge(EdgeId(0));
        let list = Value::List(Vec::new());
        let s = Value::Str("a".into());
        let b = Value::Bool(false);
        let n = Value::Int(0);
        assert_eq!(cmp_values(&map, &node), Ordering::Less);
        assert_eq!(cmp_values(&node, &edge), Ordering::Less);
        assert_eq!(cmp_values(&edge, &list), Ordering::Less);
        assert_eq!(cmp_values(&list, &s), Ordering::Less);
        assert_eq!(cmp_values(&s, &b), Ordering::Less);
        assert_eq!(cmp_values(&b, &n), Ordering::Less);
        assert_eq!(cmp_values(&n, &Value::Null), Ordering::Less);
    }

    #[test]
    fn cmp_values_int_int_and_float_float() {
        assert_eq!(cmp_values(&Value::Int(1), &Value::Int(2)), Ordering::Less);
        assert_eq!(cmp_values(&Value::Int(5), &Value::Int(5)), Ordering::Equal);
        assert_eq!(
            cmp_values(&Value::Float(2.0), &Value::Float(1.5)),
            Ordering::Greater
        );
    }

    #[test]
    fn cmp_values_int_vs_float_numeric() {
        // Cross-numeric via safe_int_float_cmp.
        assert_eq!(
            cmp_values(&Value::Int(2), &Value::Float(2.5)),
            Ordering::Less
        );
        assert_eq!(
            cmp_values(&Value::Float(2.5), &Value::Int(2)),
            Ordering::Greater
        );
    }

    #[test]
    fn cmp_values_string_lex() {
        assert_eq!(
            cmp_values(&Value::Str("apple".into()), &Value::Str("banana".into())),
            Ordering::Less
        );
    }

    #[test]
    fn cmp_values_dates_chronological() {
        let d1 = Value::Date(jiff::civil::date(2024, 1, 1));
        let d2 = Value::Date(jiff::civil::date(2024, 6, 1));
        assert_eq!(cmp_values(&d1, &d2), Ordering::Less);
    }

    #[test]
    fn cmp_values_localdatetime_chronological() {
        let a = Value::LocalDateTime(jiff::civil::datetime(2024, 1, 1, 0, 0, 0, 0));
        let b = Value::LocalDateTime(jiff::civil::datetime(2024, 1, 2, 0, 0, 0, 0));
        assert_eq!(cmp_values(&a, &b), Ordering::Less);
    }

    #[test]
    fn cmp_values_duration_lex_components() {
        use crate::types::CypherDuration;
        let a = Value::Duration(CypherDuration {
            months: 0,
            days: 1,
            nanos: 0,
        });
        let b = Value::Duration(CypherDuration {
            months: 0,
            days: 2,
            nanos: 0,
        });
        assert_eq!(cmp_values(&a, &b), Ordering::Less);
        let c = Value::Duration(CypherDuration {
            months: 1,
            days: 0,
            nanos: 0,
        });
        assert_eq!(cmp_values(&a, &c), Ordering::Less);
    }

    #[test]
    fn cmp_values_node_by_id() {
        use crate::storage::NodeId;
        assert_eq!(
            cmp_values(&Value::Node(NodeId(1)), &Value::Node(NodeId(2))),
            Ordering::Less
        );
    }

    #[test]
    fn cmp_values_localtime_vs_zoned_time_consistent() {
        let lt = Value::LocalTime(jiff::civil::time(10, 0, 0, 0));
        let zt = Value::Time(jiff::civil::time(10, 0, 0, 0), 0);
        assert_eq!(cmp_values(&lt, &zt), Ordering::Equal);
    }

    #[test]
    fn cmp_values_map_by_len() {
        let mut a = indexmap::IndexMap::new();
        a.insert(Box::from("k"), Value::Int(1));
        let b = indexmap::IndexMap::new();
        assert_eq!(cmp_values(&Value::Map(b), &Value::Map(a)), Ordering::Less);
    }

    // ── gremlin_cmp_values ─────────────────────────────────────────────────

    #[test]
    fn gremlin_cmp_null_first() {
        // Gremlin: Null < Bool < Number < ...
        assert_eq!(
            gremlin_cmp_values(&Value::Null, &Value::Bool(false)),
            Ordering::Less
        );
        assert_eq!(
            gremlin_cmp_values(&Value::Bool(false), &Value::Int(0)),
            Ordering::Less
        );
        assert_eq!(
            gremlin_cmp_values(&Value::Int(0), &Value::Str("a".into())),
            Ordering::Less
        );
    }

    #[test]
    fn gremlin_cmp_property_map_by_key_then_value() {
        use crate::storage::EdgeId;
        let mut m1 = indexmap::IndexMap::new();
        m1.insert(Box::from("~key"), Value::Str("a".into()));
        m1.insert(Box::from("~value"), Value::Int(1));
        m1.insert(Box::from("~element"), Value::Edge(EdgeId(1)));
        let mut m2 = indexmap::IndexMap::new();
        m2.insert(Box::from("~key"), Value::Str("b".into()));
        m2.insert(Box::from("~value"), Value::Int(0));
        m2.insert(Box::from("~element"), Value::Edge(EdgeId(99)));
        // Edge property compare: by key first; "a" < "b".
        assert_eq!(
            gremlin_cmp_values(&Value::Map(m1), &Value::Map(m2)),
            Ordering::Less
        );
    }

    #[test]
    fn civil_time_to_nanos_zero_and_nonzero() {
        let zero = jiff::civil::time(0, 0, 0, 0);
        assert_eq!(civil_time_to_nanos(&zero), 0);
        let one_hour = jiff::civil::time(1, 0, 0, 0);
        assert_eq!(civil_time_to_nanos(&one_hour), 3_600_000_000_000);
    }

    // ── Additional cmp_values branches ────────────────────────────────────

    #[test]
    fn cmp_values_int_vs_int() {
        assert_eq!(cmp_values(&Value::Int(1), &Value::Int(2)), Ordering::Less);
        assert_eq!(cmp_values(&Value::Int(5), &Value::Int(5)), Ordering::Equal);
        assert_eq!(
            cmp_values(&Value::Int(7), &Value::Int(2)),
            Ordering::Greater
        );
    }

    #[test]
    fn cmp_values_float_vs_float_normal() {
        assert_eq!(
            cmp_values(&Value::Float(1.0), &Value::Float(2.0)),
            Ordering::Less
        );
    }

    #[test]
    fn cmp_values_int_vs_float_and_reverse() {
        assert_eq!(
            cmp_values(&Value::Int(1), &Value::Float(2.0)),
            Ordering::Less
        );
        assert_eq!(
            cmp_values(&Value::Float(3.0), &Value::Int(2)),
            Ordering::Greater
        );
        assert_eq!(
            cmp_values(&Value::Int(5), &Value::Float(5.0)),
            Ordering::Equal
        );
    }

    #[test]
    fn cmp_values_bool_ordering() {
        assert_eq!(
            cmp_values(&Value::Bool(false), &Value::Bool(true)),
            Ordering::Less
        );
    }

    #[test]
    fn cmp_values_string_ordering() {
        assert_eq!(
            cmp_values(&Value::Str("apple".into()), &Value::Str("banana".into())),
            Ordering::Less
        );
    }

    #[test]
    fn cmp_values_date_ordering() {
        let d1 = Value::Date(jiff::civil::date(2024, 1, 1));
        let d2 = Value::Date(jiff::civil::date(2024, 6, 1));
        assert_eq!(cmp_values(&d1, &d2), Ordering::Less);
    }

    #[test]
    fn cmp_values_localtime_ordering() {
        let t1 = Value::LocalTime(jiff::civil::time(8, 0, 0, 0));
        let t2 = Value::LocalTime(jiff::civil::time(20, 0, 0, 0));
        assert_eq!(cmp_values(&t1, &t2), Ordering::Less);
    }

    #[test]
    fn cmp_values_localdatetime_ordering() {
        let d1 = Value::LocalDateTime(jiff::civil::datetime(2024, 1, 1, 0, 0, 0, 0));
        let d2 = Value::LocalDateTime(jiff::civil::datetime(2024, 6, 1, 0, 0, 0, 0));
        assert_eq!(cmp_values(&d1, &d2), Ordering::Less);
    }

    #[test]
    fn cmp_values_list_length_tiebreak() {
        let a = Value::List(vec![Value::Int(1)]);
        let b = Value::List(vec![Value::Int(1), Value::Int(2)]);
        assert_eq!(cmp_values(&a, &b), Ordering::Less);
    }

    #[test]
    fn cmp_values_list_empty_equal() {
        assert_eq!(
            cmp_values(&Value::List(vec![]), &Value::List(vec![])),
            Ordering::Equal
        );
    }

    #[test]
    fn cmp_values_map_by_length() {
        let mut m1 = indexmap::IndexMap::new();
        m1.insert(Box::from("a"), Value::Int(1));
        let mut m2 = indexmap::IndexMap::new();
        m2.insert(Box::from("a"), Value::Int(1));
        m2.insert(Box::from("b"), Value::Int(2));
        assert_eq!(cmp_values(&Value::Map(m1), &Value::Map(m2)), Ordering::Less);
    }

    #[test]
    fn cmp_values_node_by_id_v2() {
        use crate::storage::NodeId;
        assert_eq!(
            cmp_values(&Value::Node(NodeId(1)), &Value::Node(NodeId(2))),
            Ordering::Less
        );
    }

    #[test]
    fn cmp_values_edge_by_id() {
        use crate::storage::EdgeId;
        assert_eq!(
            cmp_values(&Value::Edge(EdgeId(1)), &Value::Edge(EdgeId(2))),
            Ordering::Less
        );
    }

    #[test]
    fn cmp_values_duration_ordering() {
        use crate::types::CypherDuration;
        let d1 = Value::Duration(CypherDuration {
            months: 0,
            days: 1,
            nanos: 0,
        });
        let d2 = Value::Duration(CypherDuration {
            months: 0,
            days: 2,
            nanos: 0,
        });
        assert_eq!(cmp_values(&d1, &d2), Ordering::Less);
        let d3 = Value::Duration(CypherDuration {
            months: 1,
            days: 0,
            nanos: 0,
        });
        let d4 = Value::Duration(CypherDuration {
            months: 0,
            days: 999,
            nanos: 0,
        });
        assert_eq!(cmp_values(&d3, &d4), Ordering::Greater);
    }
}
