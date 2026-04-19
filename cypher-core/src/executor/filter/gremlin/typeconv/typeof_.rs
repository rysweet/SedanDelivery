//! `type_of` introspection.

use crate::error::GqlError;
use crate::types::Value;

pub(crate) fn type_of(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Int(_)) => Ok(Value::Str("integer".into())),
        Some(Value::Float(_)) => Ok(Value::Str("double".into())),
        Some(Value::Str(_)) => Ok(Value::Str("string".into())),
        Some(Value::Bool(_)) => Ok(Value::Str("boolean".into())),
        Some(Value::Null) => Ok(Value::Str("null".into())),
        Some(Value::List(_)) => Ok(Value::Str("list".into())),
        Some(Value::Map(_)) => Ok(Value::Str("map".into())),
        Some(Value::Node(_)) => Ok(Value::Str("vertex".into())),
        Some(Value::Edge(_)) => Ok(Value::Str("edge".into())),
        Some(Value::Date(_)) => Ok(Value::Str("date".into())),
        Some(Value::Path { .. }) => Ok(Value::Str("path".into())),
        _ => Ok(Value::Str("unknown".into())),
    }
}
