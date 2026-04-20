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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{EdgeId, NodeId};

    #[test]
    fn parses_scalar_literals() {
        assert_eq!(map_key_to_value("null"), Value::Null);
        assert_eq!(map_key_to_value("true"), Value::Bool(true));
        assert_eq!(map_key_to_value("false"), Value::Bool(false));
        assert_eq!(map_key_to_value("42"), Value::Int(42));
        assert_eq!(map_key_to_value("-7"), Value::Int(-7));
        // Float requires '.' / 'e' / 'E'.
        if let Value::Float(f) = map_key_to_value("3.5") {
            assert!((f - 3.5).abs() < 1e-12);
        } else {
            panic!("expected Float");
        }
        if let Value::Float(f) = map_key_to_value("1e2") {
            assert!((f - 100.0).abs() < 1e-12);
        } else {
            panic!("expected Float");
        }
        // Non-parseable falls through to a string.
        assert_eq!(
            map_key_to_value("hello world"),
            Value::Str("hello world".into()),
        );
    }

    #[test]
    fn parses_node_and_edge_ids() {
        assert_eq!(map_key_to_value("v[12]"), Value::Node(NodeId(12)));
        assert_eq!(map_key_to_value("e[34]"), Value::Edge(EdgeId(34)));
        assert_eq!(
            map_key_to_value("Node(NodeId(99))"),
            Value::Node(NodeId(99)),
        );
        assert_eq!(map_key_to_value("Edge(EdgeId(7))"), Value::Edge(EdgeId(7)),);
        // Malformed numeric IDs fall through to string.
        assert_eq!(map_key_to_value("v[abc]"), Value::Str("v[abc]".into()),);
    }

    #[test]
    fn parses_empty_collections() {
        assert!(matches!(map_key_to_value("[]"), Value::List(ref v) if v.is_empty()));
        assert!(matches!(map_key_to_value("{}"), Value::Map(ref m) if m.is_empty()));
    }

    #[test]
    fn parses_simple_list_of_scalars() {
        let v = map_key_to_value("[1, 2, 3]");
        assert_eq!(
            v,
            Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        );
    }

    #[test]
    fn parses_nested_list_with_top_level_split() {
        // Inner brackets must NOT be split on by the comma inside.
        let v = map_key_to_value("[[1, 2], [3, 4]]");
        match v {
            Value::List(items) => {
                assert_eq!(items.len(), 2);
                assert!(matches!(&items[0], Value::List(_)));
                assert!(matches!(&items[1], Value::List(_)));
            }
            _ => panic!("expected nested list"),
        }
    }

    #[test]
    fn parses_simple_map() {
        let v = map_key_to_value("{a=1, b=2}");
        match v {
            Value::Map(m) => {
                assert_eq!(m.get("a"), Some(&Value::Int(1)));
                assert_eq!(m.get("b"), Some(&Value::Int(2)));
            }
            _ => panic!("expected Map"),
        }
    }

    #[test]
    fn parses_map_with_nested_list_value() {
        let v = map_key_to_value("{xs=[1, 2, 3], y=true}");
        match v {
            Value::Map(m) => {
                assert!(matches!(m.get("xs"), Some(Value::List(_))));
                assert_eq!(m.get("y"), Some(&Value::Bool(true)));
            }
            _ => panic!("expected Map"),
        }
    }

    #[test]
    fn map_entries_without_equals_are_skipped() {
        // No '=' inside an entry → silently dropped.
        let v = map_key_to_value("{plain}");
        if let Value::Map(m) = v {
            assert!(m.is_empty());
        } else {
            panic!("expected Map");
        }
    }
}
