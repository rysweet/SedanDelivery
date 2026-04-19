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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::Str(v.into())
    }

    #[test]
    fn containing_basic() {
        assert_eq!(
            containing(vec![s("hello"), s("ell")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            containing(vec![s("hello"), s("xyz")]).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(containing(vec![Value::Null, s("x")]).unwrap(), Value::Null);
        assert_eq!(containing(vec![s("x"), Value::Null]).unwrap(), Value::Null);
        assert_eq!(
            containing(vec![Value::Int(1), Value::Int(2)]).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(containing(vec![]).unwrap(), Value::Bool(false));
    }

    #[test]
    fn startingwith_endingwith_basic() {
        assert_eq!(
            startingwith(vec![s("hello"), s("he")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            startingwith(vec![s("hello"), s("ho")]).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            startingwith(vec![Value::Null, s("h")]).unwrap(),
            Value::Null
        );
        assert_eq!(
            startingwith(vec![s("x"), Value::Null]).unwrap(),
            Value::Null
        );
        assert_eq!(
            startingwith(vec![Value::Int(1), Value::Int(2)]).unwrap(),
            Value::Bool(false)
        );

        assert_eq!(
            endingwith(vec![s("hello"), s("lo")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            endingwith(vec![s("hello"), s("la")]).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(endingwith(vec![Value::Null, s("x")]).unwrap(), Value::Null);
        assert_eq!(endingwith(vec![s("x"), Value::Null]).unwrap(), Value::Null);
        assert_eq!(
            endingwith(vec![Value::Int(1), Value::Int(2)]).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn regex_basic() {
        assert_eq!(
            regex(vec![s("abc123"), s(r"\d+")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(regex(vec![s("abc"), s(r"^x")]).unwrap(), Value::Bool(false));
        // Invalid regex returns false
        assert_eq!(
            regex(vec![s("abc"), s(r"[invalid")]).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(regex(vec![Value::Null, s("x")]).unwrap(), Value::Null);
        assert_eq!(regex(vec![s("x"), Value::Null]).unwrap(), Value::Null);
        assert_eq!(
            regex(vec![Value::Int(1), Value::Int(2)]).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn istype_known_and_aliases() {
        assert_eq!(
            istype(vec![Value::Int(1), s("integer")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::Int(1), s("long")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::Int(1), s("int")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::Int(1), s("bigint")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::Int(1), s("biginteger")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::Float(1.0), s("double")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::Float(1.0), s("float")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::Float(1.0), s("bigdecimal")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![s("x"), s("string")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::Bool(true), s("boolean")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::Null, s("null")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::List(vec![]), s("list")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::List(vec![]), s("set")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::List(vec![]), s("path")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::Map(IndexMap::new()), s("map")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            istype(vec![Value::Map(IndexMap::new()), s("tree")]).unwrap(),
            Value::Bool(true)
        );
        // Unknown type → error
        assert!(istype(vec![Value::Int(1), s("frobnicator")]).is_err());
        // Mismatched
        assert_eq!(
            istype(vec![Value::Int(1), s("string")]).unwrap(),
            Value::Bool(false)
        );
        // Bad arg shape
        assert_eq!(istype(vec![Value::Int(1)]).unwrap(), Value::Bool(false));
    }
}
