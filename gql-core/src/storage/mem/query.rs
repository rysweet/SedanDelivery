//! Read-only query helpers for [`InMemoryGraphStore`].
//!
//! Contains: value comparison helpers and the `matches_range` function used
//! for fallback (non-indexed) property scans.

use crate::storage::RangeQuery;
use crate::types::Value;

// ── Value comparison helpers for RangeQuery ───────────────────────────────────

/// Total order for Value for range queries.  Returns `None` when the values are
/// incomparable (e.g. different types, or either is Null).
fn value_partial_cmp(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.partial_cmp(y),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y),
        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y),
        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)),
        (Value::Str(x), Value::Str(y)) => Some(x.as_ref().cmp(y.as_ref())),
        (Value::Bool(x), Value::Bool(y)) => x.partial_cmp(y),
        _ => None,
    }
}

/// Check whether a property value matches the given range predicate.
///
/// Used as the fallback path when no BTreeMap index exists for the
/// (label, property) pair.
pub(crate) fn matches_range(prop_val: &Value, range: &RangeQuery<'_>) -> bool {
    match range {
        RangeQuery::Eq(target) => prop_val.cypher_eq(target).unwrap_or(false),
        RangeQuery::Lt(bound) => {
            matches!(
                value_partial_cmp(prop_val, bound),
                Some(std::cmp::Ordering::Less)
            )
        }
        RangeQuery::Lte(bound) => matches!(
            value_partial_cmp(prop_val, bound),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ),
        RangeQuery::Gt(bound) => {
            matches!(
                value_partial_cmp(prop_val, bound),
                Some(std::cmp::Ordering::Greater)
            )
        }
        RangeQuery::Gte(bound) => matches!(
            value_partial_cmp(prop_val, bound),
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ),
        RangeQuery::Between { lo, hi, inclusive } => {
            let ge_lo = matches!(
                value_partial_cmp(prop_val, lo),
                Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
            );
            if !ge_lo {
                return false;
            }
            if *inclusive {
                matches!(
                    value_partial_cmp(prop_val, hi),
                    Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                )
            } else {
                matches!(
                    value_partial_cmp(prop_val, hi),
                    Some(std::cmp::Ordering::Less)
                )
            }
        }
        RangeQuery::StartsWith(prefix) => {
            if let Value::Str(s) = prop_val {
                s.starts_with(prefix)
            } else {
                false
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use std::collections::HashMap;

    use crate::storage::mem::test_helpers::make_store;
    use crate::storage::{GraphStore, RangeQuery};
    use crate::types::Value;

    // ── Label scanning ────────────────────────────────────────────────────────

    #[test]
    fn label_scan_returns_correct_nodes() {
        let store = make_store();
        let a = store
            .create_node("test", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("test", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        let _c = store
            .create_node("test", vec!["Company".to_string()], IndexMap::new())
            .unwrap();

        let mut persons = store.find_nodes_by_label("test", "Person").unwrap();
        persons.sort();
        assert_eq!(persons, vec![a, b]);

        let companies = store.find_nodes_by_label("test", "Company").unwrap();
        assert_eq!(companies.len(), 1);

        let missing = store.find_nodes_by_label("test", "DoesNotExist").unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    fn label_and_property_scan_eq() {
        let store = make_store();
        let mut p1 = HashMap::new();
        p1.insert("age".to_string(), Value::Int(30));
        let a = store
            .create_node("test", vec!["Person".to_string()], p1)
            .unwrap();

        let mut p2 = HashMap::new();
        p2.insert("age".to_string(), Value::Int(25));
        let _b = store
            .create_node("test", vec!["Person".to_string()], p2)
            .unwrap();

        let bound = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert_eq!(result, vec![a]);
    }

    #[test]
    fn label_and_property_scan_range() {
        let store = make_store();
        let make_person = |age: i64| {
            let mut p = HashMap::new();
            p.insert("age".to_string(), Value::Int(age));
            p
        };
        let a = store
            .create_node("test", vec!["Person".to_string()], make_person(20))
            .unwrap();
        let b = store
            .create_node("test", vec!["Person".to_string()], make_person(30))
            .unwrap();
        let _c = store
            .create_node("test", vec!["Person".to_string()], make_person(40))
            .unwrap();

        let lo = Value::Int(20);
        let hi = Value::Int(30);
        let mut result = store
            .find_nodes_by_label_and_property(
                "test",
                "Person",
                "age",
                RangeQuery::Between {
                    lo: &lo,
                    hi: &hi,
                    inclusive: true,
                },
            )
            .unwrap();
        result.sort();
        assert_eq!(result, vec![a, b]);
    }

    #[test]
    fn label_and_property_scan_starts_with() {
        let store = make_store();
        let mut p1 = HashMap::new();
        p1.insert("name".to_string(), Value::Str("Alice".into()));
        let a = store
            .create_node("test", vec!["Person".to_string()], p1)
            .unwrap();

        let mut p2 = HashMap::new();
        p2.insert("name".to_string(), Value::Str("Bob".into()));
        let _b = store
            .create_node("test", vec!["Person".to_string()], p2)
            .unwrap();

        let result = store
            .find_nodes_by_label_and_property(
                "test",
                "Person",
                "name",
                RangeQuery::StartsWith("Al"),
            )
            .unwrap();
        assert_eq!(result, vec![a]);
    }

    // ── Adjacency ─────────────────────────────────────────────────────────────

    #[test]
    fn adjacency_out_and_in() {
        let store = make_store();
        let a = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let b = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let eid = store
            .create_edge("test", a, b, "KNOWS".to_string(), IndexMap::new())
            .unwrap();

        let out = store.out_edges("test", a).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].edge_id, eid);
        assert_eq!(out[0].neighbor_id, b);
        assert_eq!(out[0].rel_type, "KNOWS");

        let in_edges = store.in_edges("test", b).unwrap();
        assert_eq!(in_edges.len(), 1);
        assert_eq!(in_edges[0].edge_id, eid);
        assert_eq!(in_edges[0].neighbor_id, a);

        assert!(store.in_edges("test", a).unwrap().is_empty());
        assert!(store.out_edges("test", b).unwrap().is_empty());
    }

    #[test]
    fn out_edges_by_type_filters() {
        let store = make_store();
        let a = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let b = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let c = store.create_node("test", vec![], IndexMap::new()).unwrap();

        store
            .create_edge("test", a, b, "KNOWS".to_string(), IndexMap::new())
            .unwrap();
        store
            .create_edge("test", a, c, "LIKES".to_string(), IndexMap::new())
            .unwrap();

        let knows = store.out_edges_by_type("test", a, &["KNOWS"]).unwrap();
        assert_eq!(knows.len(), 1);
        assert_eq!(knows[0].neighbor_id, b);

        let all = store.out_edges_by_type("test", a, &[]).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn in_edges_by_type_filters() {
        let store = make_store();
        let a = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let b = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let c = store.create_node("test", vec![], IndexMap::new()).unwrap();

        store
            .create_edge("test", a, c, "KNOWS".to_string(), IndexMap::new())
            .unwrap();
        store
            .create_edge("test", b, c, "LIKES".to_string(), IndexMap::new())
            .unwrap();

        let knows_in = store.in_edges_by_type("test", c, &["KNOWS"]).unwrap();
        assert_eq!(knows_in.len(), 1);
        assert_eq!(knows_in[0].neighbor_id, a);
    }

    // ── find_edges_by_type ────────────────────────────────────────────────────

    #[test]
    fn find_edges_by_type_returns_correct_edges() {
        let store = make_store();
        let a = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let b = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let c = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let e1 = store
            .create_edge("test", a, b, "KNOWS".to_string(), IndexMap::new())
            .unwrap();
        let _e2 = store
            .create_edge("test", b, c, "LIKES".to_string(), IndexMap::new())
            .unwrap();

        let knows = store.find_edges_by_type("test", "KNOWS").unwrap();
        assert_eq!(knows, vec![e1]);
    }

    // ── out_edge_ids / in_edge_ids ────────────────────────────────────────────

    #[test]
    fn out_edge_ids_and_in_edge_ids() {
        let store = make_store();
        let a = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let b = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let eid = store
            .create_edge("test", a, b, "R".to_string(), IndexMap::new())
            .unwrap();

        let out_ids = store.out_edge_ids("test", a).unwrap();
        assert_eq!(out_ids, vec![eid]);
        let in_ids = store.in_edge_ids("test", b).unwrap();
        assert_eq!(in_ids, vec![eid]);
    }
}
