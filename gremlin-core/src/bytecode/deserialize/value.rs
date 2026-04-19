//! Raw JSON → [`GremlinValue`] conversion plus argument collectors used by
//! the step dispatch table.

use crate::types::GremlinValue;

pub(in crate::bytecode) fn json_to_gremlin_value(v: &serde_json::Value) -> GremlinValue {
    match v {
        serde_json::Value::Null => GremlinValue::Null,
        serde_json::Value::Bool(b) => GremlinValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                GremlinValue::Integer(i)
            } else {
                GremlinValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => GremlinValue::String(s.clone()),
        serde_json::Value::Array(arr) => {
            GremlinValue::List(arr.iter().map(json_to_gremlin_value).collect())
        }
        serde_json::Value::Object(obj) => {
            let map = obj
                .iter()
                .map(|(k, v)| (k.clone(), json_to_gremlin_value(v)))
                .collect();
            GremlinValue::Map(map)
        }
    }
}

pub(in crate::bytecode) fn collect_string_args(args: &[serde_json::Value]) -> Vec<String> {
    args.iter()
        .filter_map(|a| a.as_str().map(String::from))
        .collect()
}

pub(in crate::bytecode) fn collect_value_args(args: &[serde_json::Value]) -> Vec<GremlinValue> {
    args.iter().map(json_to_gremlin_value).collect()
}
