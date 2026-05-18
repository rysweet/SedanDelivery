//! Helpers for converting `Value`s to Gremlin-compatible string forms,
//! merging map entries with Gremlin's group-by semantics, and ad-hoc
//! debug-shape summaries / file logging.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

/// Convert a Value to a string key for use in Gremlin group() Map output.
pub(in crate::executor::engine::gremlin) fn value_to_map_key(v: &Value) -> Box<str> {
    match v {
        Value::Str(s) => s.clone(),
        Value::Int(i) => format!("{i}").into_boxed_str(),
        Value::Float(f) => {
            let s = format!("{f}");
            // Ensure floats always have a decimal point for consistent key format
            if s.contains('.') || s.contains('E') || s.contains('e') {
                s.into_boxed_str()
            } else {
                format!("{s}.0").into_boxed_str()
            }
        }
        Value::Bool(b) => format!("{b}").into_boxed_str(),
        Value::Null => "null".to_string().into_boxed_str(),
        Value::Node(nid) => format!("v[{}]", nid.0).into_boxed_str(),
        Value::Edge(eid) => format!("e[{}]", eid.0).into_boxed_str(),
        Value::List(items) => {
            // Gremlin list key format: l[elem1,elem2,...] (no spaces)
            let parts: Vec<String> = items
                .iter()
                .map(|item| value_to_map_key(item).to_string())
                .collect();
            format!("l[{}]", parts.join(",")).into_boxed_str()
        }
        Value::Map(m) => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("{k}={}", value_to_map_key(v)))
                .collect();
            format!("{{{}}}", parts.join(", ")).into_boxed_str()
        }
        _ => format!("{v:?}").into_boxed_str(),
    }
}

pub(in crate::executor::engine::gremlin) fn merge_gremlin_map_entry(
    map: &mut IndexMap<Box<str>, Value>,
    key: &str,
    incoming: &Value,
) -> Result<(), GqlError> {
    match map.entry(Box::from(key)) {
        indexmap::map::Entry::Vacant(entry) => {
            entry.insert(incoming.clone());
        }
        indexmap::map::Entry::Occupied(mut entry) => match (entry.get().clone(), incoming) {
            (Value::Int(a), Value::Int(b)) => {
                entry.insert(Value::Int(a + b));
            }
            (Value::Float(a), Value::Float(b)) => {
                entry.insert(Value::Float(a + b));
            }
            (Value::List(mut a), Value::List(b)) => {
                a.extend(b.iter().cloned());
                entry.insert(Value::List(a));
            }
            (ref existing, other) => {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: format!("{:?}", std::mem::discriminant(existing)),
                    actual: format!("{:?}", std::mem::discriminant(other)),
                }));
            }
        },
    }
    Ok(())
}

pub(in crate::executor::engine::gremlin) fn summarize_gremlin_value_shape(
    value: &Value,
    depth: usize,
) -> String {
    if depth == 0 {
        return "...".to_string();
    }
    match value {
        Value::List(items) => {
            let sample = items
                .iter()
                .take(3)
                .map(|item| summarize_gremlin_value_shape(item, depth - 1))
                .collect::<Vec<_>>()
                .join(", ");
            format!("list(len={}, sample=[{}])", items.len(), sample)
        }
        Value::Map(entries) => {
            let sample = entries
                .iter()
                .take(2)
                .map(|(k, v)| format!("{k}:{}", summarize_gremlin_value_shape(v, depth - 1)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("map(len={}, sample={{{}}})", entries.len(), sample)
        }
        Value::Node(nid) => format!("node({})", nid.0),
        Value::Edge(eid) => format!("edge({})", eid.0),
        Value::Str(s) => format!("str({s})"),
        Value::Int(i) => format!("int({i})"),
        Value::Float(f) => format!("float({f})"),
        Value::Bool(b) => format!("bool({b})"),
        Value::Null => "null".to_string(),
        other => format!("{other:?}"),
    }
}

pub(in crate::executor::engine::gremlin) fn append_gremlin_debug_log(line: &str) {
    use std::io::Write;

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/sd-gremlin-debug.log")
    {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ExecutionError, GqlError};

    // ── Bug #1206: merge_gremlin_map_entry must error on cross-type values ──
    //
    // Currently the `(_, other)` catch-all arm silently overwrites the
    // existing value with the incoming value when types don't match
    // (e.g., Int + Str). After fix: return Result<(), GqlError> and error
    // with ExecutionError::TypeMismatch for incompatible types.

    #[test]
    fn merge_same_type_int_adds() {
        let mut map = IndexMap::new();
        map.insert(Box::from("k"), Value::Int(10));
        let result = merge_gremlin_map_entry(&mut map, "k", &Value::Int(5));
        assert!(result.is_ok());
        assert_eq!(map.get("k"), Some(&Value::Int(15)));
    }

    #[test]
    fn merge_same_type_float_adds() {
        let mut map = IndexMap::new();
        map.insert(Box::from("k"), Value::Float(1.5));
        let result = merge_gremlin_map_entry(&mut map, "k", &Value::Float(2.5));
        assert!(result.is_ok());
        assert_eq!(map.get("k"), Some(&Value::Float(4.0)));
    }

    #[test]
    fn merge_list_extends() {
        let mut map = IndexMap::new();
        map.insert(Box::from("k"), Value::List(vec![Value::Int(1)]));
        let result = merge_gremlin_map_entry(&mut map, "k", &Value::List(vec![Value::Int(2)]));
        assert!(result.is_ok());
        assert_eq!(
            map.get("k"),
            Some(&Value::List(vec![Value::Int(1), Value::Int(2)]))
        );
    }

    #[test]
    fn merge_vacant_entry_inserts() {
        let mut map = IndexMap::new();
        let result = merge_gremlin_map_entry(&mut map, "k", &Value::Int(42));
        assert!(result.is_ok());
        assert_eq!(map.get("k"), Some(&Value::Int(42)));
    }

    #[test]
    fn merge_cross_type_int_str_returns_error() {
        // Int existing + Str incoming → should be a TypeMismatch error.
        let mut map = IndexMap::new();
        map.insert(Box::from("k"), Value::Int(10));
        let result = merge_gremlin_map_entry(&mut map, "k", &Value::Str("hello".into()));
        assert!(
            result.is_err(),
            "merging Int with Str should return error, got: {result:?}"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(
                err,
                GqlError::Execution(ExecutionError::TypeMismatch { .. })
            ),
            "expected TypeMismatch, got: {err:?}"
        );
    }

    #[test]
    fn merge_cross_type_float_int_returns_error() {
        // Float existing + Int incoming → should be a TypeMismatch error.
        let mut map = IndexMap::new();
        map.insert(Box::from("k"), Value::Float(1.5));
        let result = merge_gremlin_map_entry(&mut map, "k", &Value::Int(2));
        assert!(
            result.is_err(),
            "merging Float with Int should return error, got: {result:?}"
        );
    }

    #[test]
    fn merge_cross_type_str_int_returns_error() {
        // Str existing + Int incoming → should be a TypeMismatch error.
        let mut map = IndexMap::new();
        map.insert(Box::from("k"), Value::Str("hello".into()));
        let result = merge_gremlin_map_entry(&mut map, "k", &Value::Int(1));
        assert!(
            result.is_err(),
            "merging Str with Int should return error, got: {result:?}"
        );
    }

    // ── value_to_map_key regression tests ──────────────────────────────

    #[test]
    fn value_to_map_key_int() {
        assert_eq!(&*value_to_map_key(&Value::Int(42)), "42");
    }

    #[test]
    fn value_to_map_key_null() {
        assert_eq!(&*value_to_map_key(&Value::Null), "null");
    }
}
