//! Gremlin predicates functions extracted from filter::eval_function_with_vals.
//!
//! Behavior preserved verbatim from the original `filter` module.

#![allow(clippy::needless_pass_by_value)]
#![allow(unused_imports)]

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn containing(vals: Vec<Value>) -> Result<Value, GqlError> {
    match (vals.first(), vals.get(1)) {
        (Some(Value::Str(haystack)), Some(Value::Str(needle))) => {
            Ok(Value::Bool(haystack.contains(needle.as_ref())))
        }
        (Some(Value::Null), _) | (_, Some(Value::Null)) => Ok(Value::Null),
        _ => Ok(Value::Bool(false)),
    }
}

pub(crate) fn startingwith(vals: Vec<Value>) -> Result<Value, GqlError> {
    match (vals.first(), vals.get(1)) {
        (Some(Value::Str(haystack)), Some(Value::Str(prefix))) => {
            Ok(Value::Bool(haystack.starts_with(prefix.as_ref())))
        }
        (Some(Value::Null), _) | (_, Some(Value::Null)) => Ok(Value::Null),
        _ => Ok(Value::Bool(false)),
    }
}

pub(crate) fn endingwith(vals: Vec<Value>) -> Result<Value, GqlError> {
    match (vals.first(), vals.get(1)) {
        (Some(Value::Str(haystack)), Some(Value::Str(suffix))) => {
            Ok(Value::Bool(haystack.ends_with(suffix.as_ref())))
        }
        (Some(Value::Null), _) | (_, Some(Value::Null)) => Ok(Value::Null),
        _ => Ok(Value::Bool(false)),
    }
}

pub(crate) fn regex(vals: Vec<Value>) -> Result<Value, GqlError> {
    match (vals.first(), vals.get(1)) {
        (Some(Value::Str(haystack)), Some(Value::Str(pattern))) => {
            match regex::Regex::new(pattern.as_ref()) {
                Ok(re) => Ok(Value::Bool(re.is_match(haystack))),
                Err(_) => Ok(Value::Bool(false)),
            }
        }
        (Some(Value::Null), _) | (_, Some(Value::Null)) => Ok(Value::Null),
        _ => Ok(Value::Bool(false)),
    }
}

pub(crate) fn istype(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        match (vals.first(), vals.get(1)) {
            (Some(val), Some(Value::Str(expected_type))) => {
                let actual_type = match val {
                    Value::Int(_) => "integer",
                    Value::Float(_) => "double",
                    Value::Str(_) => "string",
                    Value::Bool(_) => "boolean",
                    Value::Null => "null",
                    Value::List(_) => "list",
                    Value::Map(_) => "map",
                    Value::Node(_) => "vertex",
                    Value::Edge(_) => "edge",
                    Value::Date(_) => "date",
                    Value::Path { .. } => "path",
                    _ => "unknown",
                };
                let expected = expected_type.to_lowercase();
                // Validate that the type name is recognized
                let known_types = [
                    "string",
                    "integer",
                    "int",
                    "long",
                    "double",
                    "float",
                    "boolean",
                    "null",
                    "list",
                    "map",
                    "set",
                    "path",
                    "vertex",
                    "edge",
                    "date",
                    "short",
                    "byte",
                    "biginteger",
                    "bigdecimal",
                    "bigint",
                    "uuid",
                    "tree",
                    "binary",
                    "char",
                    "datetime",
                    "duration",
                    "graph",
                    "vproperty",
                    "property",
                ];
                if !known_types.contains(&expected.as_str()) {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: format!("a known GType, got '{}'", expected_type),
                        actual: "unregistered type name".to_string(),
                    }));
                }
                let matches = actual_type == expected
                    || (expected == "long" && actual_type == "integer")
                    || (expected == "int" && actual_type == "integer")
                    || (expected == "bigint" && actual_type == "integer")
                    || (expected == "biginteger" && actual_type == "integer")
                    || (expected == "bigdecimal" && actual_type == "double")
                    || (expected == "float" && actual_type == "double")
                    || (expected == "set" && actual_type == "list")
                    || (expected == "path" && actual_type == "list")
                    || (expected == "tree" && actual_type == "map");
                Ok(Value::Bool(matches))
            }
            _ => Ok(Value::Bool(false)),
        }
    }
}
