//! Parser converting opaque key strings into [`Value`]s.
//!
//! Used when reading map/list keys serialized as text (e.g. from older
//! storage backends or debug round-trips).  Behavior preserved verbatim
//! from the original `filter` module.

use indexmap::IndexMap;

use crate::types::Value;

pub(crate) fn map_key_to_value(k: &str) -> Value {
    use crate::storage::{EdgeId, NodeId};

    fn split_top_level(input: &str, delimiter: char) -> Vec<&str> {
        let mut parts = Vec::new();
        let mut depth_paren = 0i32;
        let mut depth_brace = 0i32;
        let mut depth_bracket = 0i32;
        let mut start = 0usize;
        for (idx, ch) in input.char_indices() {
            match ch {
                '(' => depth_paren += 1,
                ')' => depth_paren -= 1,
                '{' => depth_brace += 1,
                '}' => depth_brace -= 1,
                '[' => depth_bracket += 1,
                ']' => depth_bracket -= 1,
                _ => {}
            }
            if ch == delimiter && depth_paren == 0 && depth_brace == 0 && depth_bracket == 0 {
                parts.push(input[start..idx].trim());
                start = idx + ch.len_utf8();
            }
        }
        parts.push(input[start..].trim());
        parts
    }

    fn parse_scalar(input: &str) -> Option<Value> {
        if input == "null" {
            return Some(Value::Null);
        }
        if input == "true" {
            return Some(Value::Bool(true));
        }
        if input == "false" {
            return Some(Value::Bool(false));
        }
        if let Ok(n) = input.parse::<i64>() {
            return Some(Value::Int(n));
        }
        if input.contains(['.', 'e', 'E']) {
            if let Ok(f) = input.parse::<f64>() {
                return Some(Value::Float(f));
            }
        }
        if let Some(num) = input
            .strip_prefix("v[")
            .and_then(|rest| rest.strip_suffix(']'))
        {
            if let Ok(n) = num.parse::<u64>() {
                return Some(Value::Node(NodeId(n)));
            }
        }
        if let Some(num) = input
            .strip_prefix("e[")
            .and_then(|rest| rest.strip_suffix(']'))
        {
            if let Ok(n) = num.parse::<u64>() {
                return Some(Value::Edge(EdgeId(n)));
            }
        }
        if let Some(inner) = input
            .strip_prefix("Node(NodeId(")
            .and_then(|s| s.strip_suffix("))"))
        {
            if let Ok(id) = inner.parse::<u64>() {
                return Some(Value::Node(NodeId(id)));
            }
        }
        if let Some(inner) = input
            .strip_prefix("Edge(EdgeId(")
            .and_then(|s| s.strip_suffix("))"))
        {
            if let Ok(id) = inner.parse::<u64>() {
                return Some(Value::Edge(EdgeId(id)));
            }
        }
        None
    }

    if let Some(value) = parse_scalar(k) {
        return value;
    }
    if let Some(inner) = k.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) {
        let mut map = IndexMap::new();
        if !inner.trim().is_empty() {
            for entry in split_top_level(inner, ',') {
                if let Some((raw_key, raw_val)) = entry.split_once('=') {
                    map.insert(
                        raw_key.trim().to_string().into_boxed_str(),
                        map_key_to_value(raw_val.trim()),
                    );
                }
            }
        }
        return Value::Map(map);
    }
    if let Some(inner) = k.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
        let values = if inner.trim().is_empty() {
            Vec::new()
        } else {
            split_top_level(inner, ',')
                .into_iter()
                .map(map_key_to_value)
                .collect()
        };
        return Value::List(values);
    }
    Value::Str(k.to_string().into_boxed_str())
}
