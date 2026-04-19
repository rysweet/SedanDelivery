//! JSON ↔ `cypher_core::types::Value` conversion helpers.

use cypher_core::types::Value;
use indexmap::IndexMap;

use crate::error::ApiError;

/// Convert a `serde_json::Value` into a `cypher_core::types::Value`.
///
/// JSON numbers become `Value::Int` if they are integers, `Value::Float` otherwise.
/// Nested objects become `Value::Map`, arrays become `Value::List`.
/// `null` becomes `Value::Null`.
pub fn json_to_value(v: serde_json::Value) -> Result<Value, ApiError> {
    match v {
        serde_json::Value::Null => Ok(Value::Null),
        serde_json::Value::Bool(b) => Ok(Value::Bool(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::Float(f))
            } else {
                Err(ApiError::bad_request(format!(
                    "numeric value '{n}' out of range"
                )))
            }
        }
        serde_json::Value::String(s) => Ok(Value::Str(s.into_boxed_str())),
        serde_json::Value::Array(items) => {
            let vals: Result<Vec<Value>, ApiError> = items.into_iter().map(json_to_value).collect();
            Ok(Value::List(vals?))
        }
        serde_json::Value::Object(obj) => {
            let mut m: IndexMap<Box<str>, Value> = IndexMap::new();
            for (k, val) in obj {
                m.insert(k.into_boxed_str(), json_to_value(val)?);
            }
            Ok(Value::Map(m))
        }
    }
}

/// Convert a `serde_json::Map` of property values into an `IndexMap<String, Value>`.
pub fn json_map_to_props(
    map: serde_json::Map<String, serde_json::Value>,
) -> Result<IndexMap<String, Value>, ApiError> {
    map.into_iter()
        .map(|(k, v)| {
            let val = json_to_value(v)?;
            Ok((k, val))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── json_to_value ────────────────────────────────────────────────────────

    #[test]
    fn json_to_value_null() {
        let result = json_to_value(json!(null)).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn json_to_value_bool_true() {
        let result = json_to_value(json!(true)).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn json_to_value_bool_false() {
        let result = json_to_value(json!(false)).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn json_to_value_integer() {
        let result = json_to_value(json!(42)).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn json_to_value_negative_integer() {
        let result = json_to_value(json!(-7)).unwrap();
        assert_eq!(result, Value::Int(-7));
    }

    #[test]
    fn json_to_value_float() {
        let result = json_to_value(json!(std::f64::consts::PI)).unwrap();
        assert_eq!(result, Value::Float(std::f64::consts::PI));
    }

    #[test]
    fn json_to_value_string() {
        let result = json_to_value(json!("hello")).unwrap();
        assert_eq!(result, Value::Str("hello".into()));
    }

    #[test]
    fn json_to_value_empty_string() {
        let result = json_to_value(json!("")).unwrap();
        assert_eq!(result, Value::Str("".into()));
    }

    #[test]
    fn json_to_value_array() {
        let result = json_to_value(json!([1, "two", null])).unwrap();
        match result {
            Value::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], Value::Int(1));
                assert_eq!(items[1], Value::Str("two".into()));
                assert_eq!(items[2], Value::Null);
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn json_to_value_empty_array() {
        let result = json_to_value(json!([])).unwrap();
        match result {
            Value::List(items) => assert!(items.is_empty()),
            other => panic!("expected empty List, got {other:?}"),
        }
    }

    #[test]
    fn json_to_value_object() {
        let result = json_to_value(json!({"name": "Alice", "age": 30})).unwrap();
        match result {
            Value::Map(m) => {
                assert_eq!(m.len(), 2);
                assert_eq!(m.get("name"), Some(&Value::Str("Alice".into())));
                assert_eq!(m.get("age"), Some(&Value::Int(30)));
            }
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn json_to_value_nested_object() {
        let result = json_to_value(json!({"outer": {"inner": 1}})).unwrap();
        match result {
            Value::Map(m) => {
                let inner = m.get("outer").unwrap();
                match inner {
                    Value::Map(inner_m) => {
                        assert_eq!(inner_m.get("inner"), Some(&Value::Int(1)));
                    }
                    other => panic!("expected nested Map, got {other:?}"),
                }
            }
            other => panic!("expected Map, got {other:?}"),
        }
    }

    // ── json_map_to_props ────────────────────────────────────────────────────

    #[test]
    fn json_map_to_props_empty() {
        let map = serde_json::Map::new();
        let result = json_map_to_props(map).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn json_map_to_props_mixed_types() {
        let mut map = serde_json::Map::new();
        map.insert("name".to_string(), json!("Alice"));
        map.insert("age".to_string(), json!(30));
        map.insert("active".to_string(), json!(true));
        map.insert("score".to_string(), json!(9.5));

        let result = json_map_to_props(map).unwrap();
        assert_eq!(result.len(), 4);
        assert_eq!(result.get("name"), Some(&Value::Str("Alice".into())));
        assert_eq!(result.get("age"), Some(&Value::Int(30)));
        assert_eq!(result.get("active"), Some(&Value::Bool(true)));
        assert_eq!(result.get("score"), Some(&Value::Float(9.5)));
    }

    #[test]
    fn json_map_to_props_with_array_value() {
        let mut map = serde_json::Map::new();
        map.insert("tags".to_string(), json!(["a", "b", "c"]));

        let result = json_map_to_props(map).unwrap();
        match result.get("tags") {
            Some(Value::List(items)) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], Value::Str("a".into()));
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn json_map_to_props_with_nested_object() {
        let mut map = serde_json::Map::new();
        map.insert("meta".to_string(), json!({"key": "value"}));

        let result = json_map_to_props(map).unwrap();
        match result.get("meta") {
            Some(Value::Map(m)) => {
                assert_eq!(m.get("key"), Some(&Value::Str("value".into())));
            }
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn json_map_to_props_with_null_value() {
        let mut map = serde_json::Map::new();
        map.insert("deleted".to_string(), json!(null));

        let result = json_map_to_props(map).unwrap();
        assert_eq!(result.get("deleted"), Some(&Value::Null));
    }
}
