//! Helpers for converting `Value`s to Gremlin-compatible string forms,
//! merging map entries with Gremlin's group-by semantics, and ad-hoc
//! debug-shape summaries / file logging.

use indexmap::IndexMap;

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
) {
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
            (_, other) => {
                entry.insert(other.clone());
            }
        },
    }
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
