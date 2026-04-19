//! Map / property accessor builtins: keys, values, properties.

use crate::error::GqlError;
use crate::types::Value;

use super::helpers::{require_arity, type_error};

pub(in crate::executor::filter) fn builtin_keys(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("keys", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Map(m) => Ok(Value::List(
            m.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, _)| map_key_str_to_value(k))
                .collect(),
        )),
        other => type_error("keys", other),
    }
}

fn map_key_str_to_value(k: &str) -> Value {
    use crate::storage::{EdgeId, NodeId};

    if let Some(inner) = k.strip_prefix("v[").and_then(|s| s.strip_suffix(']')) {
        if let Ok(id) = inner.parse::<u64>() {
            return Value::Node(NodeId(id));
        }
    }
    if let Some(inner) = k.strip_prefix("e[").and_then(|s| s.strip_suffix(']')) {
        if let Ok(id) = inner.parse::<u64>() {
            return Value::Edge(EdgeId(id));
        }
    }
    Value::Str(k.into())
}

pub(in crate::executor::filter) fn builtin_values(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("values", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Map(m) => Ok(Value::List(m.values().cloned().collect())),
        other => type_error("values", other),
    }
}

pub(in crate::executor::filter) fn builtin_properties(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("properties", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Map(m) => Ok(Value::Map(m.clone())),
        other => type_error("properties", other),
    }
}
