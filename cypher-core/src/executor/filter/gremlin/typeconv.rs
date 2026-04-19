//! Gremlin typeconv functions extracted from filter::eval_function_with_vals.
//!
//! Behavior preserved verbatim from the original `filter` module.

#![allow(clippy::needless_pass_by_value)]
#![allow(unused_imports)]

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn asstring(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Str(s)) => Ok(Value::Str(s.clone())),
        Some(Value::Int(n)) => Ok(Value::Str(n.to_string().into())),
        Some(Value::Float(f)) => {
            if f.is_nan() {
                Ok(Value::Str("NaN".into()))
            } else if f.is_infinite() {
                Ok(Value::Str(
                    if *f > 0.0 { "Infinity" } else { "-Infinity" }.into(),
                ))
            } else {
                Ok(Value::Str(f.to_string().into()))
            }
        }
        Some(Value::Bool(b)) => Ok(Value::Str(b.to_string().into())),
        Some(Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "Can't parse null as string".into(),
            actual: "null".into(),
        })),
        Some(Value::List(items)) => {
            let parts: Vec<String> = items
                .iter()
                .map(|v| match v {
                    Value::Str(s) => s.to_string(),
                    Value::Int(n) => n.to_string(),
                    Value::Float(f) => f.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => "null".to_string(),
                    other => format!("{:?}", other),
                })
                .collect();
            Ok(Value::Str(format!("[{}]", parts.join(", ")).into()))
        }
        Some(Value::Map(m)) => {
            // Detect VertexProperty maps: {~key, ~value, ~element(Node)}
            if m.contains_key("~key") && m.contains_key("~value") {
                if let (Some(Value::Str(key)), Some(val)) = (m.get("~key"), m.get("~value")) {
                    let val_str = match val {
                        Value::Str(s) => s.to_string(),
                        Value::Int(n) => n.to_string(),
                        Value::Float(f) => f.to_string(),
                        Value::Bool(b) => b.to_string(),
                        other => format!("{:?}", other),
                    };
                    return Ok(Value::Str(format!("vp[{}->{}]", key, val_str).into()));
                }
            }
            let parts: Vec<String> = m
                .iter()
                .filter(|(k, _)| !k.starts_with('~'))
                .map(|(k, v)| {
                    let vs = match v {
                        Value::Str(s) => s.to_string(),
                        Value::Int(n) => n.to_string(),
                        Value::Float(f) => f.to_string(),
                        Value::Bool(b) => b.to_string(),
                        Value::List(items) => {
                            let inner: Vec<String> = items
                                .iter()
                                .map(|i| match i {
                                    Value::Str(s) => s.to_string(),
                                    Value::Int(n) => n.to_string(),
                                    other => format!("{:?}", other),
                                })
                                .collect();
                            format!("[{}]", inner.join(", "))
                        }
                        other => format!("{:?}", other),
                    };
                    format!("{}={}", k, vs)
                })
                .collect();
            Ok(Value::Str(format!("{{{}}}", parts.join(", ")).into()))
        }
        Some(Value::Node(nid)) => Ok(Value::Str(format!("v[{}]", nid.0).into())),
        Some(Value::Edge(eid)) => Ok(Value::Str(format!("e[{}]", eid.0).into())),
        Some(Value::Path { nodes, edges }) => {
            let parts: Vec<String> = nodes
                .iter()
                .map(|n| format!("v[{}]", n.0))
                .chain(edges.iter().map(|e| format!("e[{}]", e.0)))
                .collect();
            Ok(Value::Str(format!("path[{}]", parts.join(", ")).into()))
        }
        Some(other) => Ok(Value::Str(format!("{:?}", other).into())),
        None => Ok(Value::Null),
    }
}

pub(crate) fn asdate(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // asDate() — convert epoch-millis integer to Date value
        match vals.first() {
            Some(Value::Int(millis)) => {
                let secs = millis / 1000;
                let nanos = ((millis % 1000) * 1_000_000) as i32;
                let ts = jiff::Timestamp::new(secs, nanos).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
                let dt = ts.to_zoned(jiff::tz::TimeZone::UTC);
                Ok(Value::DateTime(Box::new(dt)))
            }
            Some(Value::Float(f)) => {
                let millis = *f as i64;
                let secs = millis / 1000;
                let nanos = ((millis % 1000) * 1_000_000) as i32;
                let ts = jiff::Timestamp::new(secs, nanos).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
                let dt = ts.to_zoned(jiff::tz::TimeZone::UTC);
                Ok(Value::DateTime(Box::new(dt)))
            }
            Some(Value::DateTime(d)) => Ok(Value::DateTime(d.clone())),
            Some(Value::Null) => Ok(Value::Null),
            _ => Ok(Value::Null),
        }
    }
}

pub(crate) fn asstring_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // asString(Scope.local): convert list elements to strings
        fn value_to_str(v: &Value) -> Result<Value, GqlError> {
            match v {
                Value::Str(s) => Ok(Value::Str(s.clone())),
                Value::Int(n) => Ok(Value::Str(n.to_string().into())),
                Value::Float(f) => Ok(Value::Str(f.to_string().into())),
                Value::Bool(b) => Ok(Value::Str(b.to_string().into())),
                Value::Node(nid) => Ok(Value::Str(format!("v[{}]", nid.0).into())),
                Value::Edge(eid) => Ok(Value::Str(format!("e[{}]", eid.0).into())),
                Value::Null => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Can't parse null as string".into(),
                    actual: "null".into(),
                })),
                Value::Map(m) => {
                    // Detect VertexProperty maps
                    if m.contains_key("~key") && m.contains_key("~value") {
                        if let (Some(Value::Str(key)), Some(val)) = (m.get("~key"), m.get("~value"))
                        {
                            let val_str = match val {
                                Value::Str(s) => s.to_string(),
                                Value::Int(n) => n.to_string(),
                                Value::Float(f) => f.to_string(),
                                Value::Bool(b) => b.to_string(),
                                other => format!("{:?}", other),
                            };
                            return Ok(Value::Str(format!("vp[{}->{}]", key, val_str).into()));
                        }
                    }
                    // Regular map: {key=[values]} format
                    let parts: Vec<String> = m
                        .iter()
                        .filter(|(k, _)| !k.starts_with('~'))
                        .map(|(k, v)| {
                            let vs = match v {
                                Value::Str(s) => s.to_string(),
                                Value::Int(n) => n.to_string(),
                                Value::Float(f) => f.to_string(),
                                Value::Bool(b) => b.to_string(),
                                Value::List(items) => {
                                    let inner: Vec<String> = items
                                        .iter()
                                        .map(|i| match i {
                                            Value::Str(s) => s.to_string(),
                                            Value::Int(n) => n.to_string(),
                                            other => format!("{:?}", other),
                                        })
                                        .collect();
                                    format!("[{}]", inner.join(", "))
                                }
                                other => format!("{:?}", other),
                            };
                            format!("{}={}", k, vs)
                        })
                        .collect();
                    Ok(Value::Str(format!("{{{}}}", parts.join(", ")).into()))
                }
                other => Ok(Value::Str(format!("{:?}", other).into())),
            }
        }
        match vals.first() {
            Some(Value::List(items)) => {
                let converted: Result<Vec<Value>, GqlError> =
                    items.iter().map(value_to_str).collect();
                Ok(Value::List(converted?))
            }
            Some(other) => value_to_str(other),
            None => Ok(Value::Null),
        }
    }
}

pub(crate) fn asnumber(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Int(n)) => Ok(Value::Int(*n)),
        Some(Value::Float(f)) => Ok(Value::Float(*f)),
        Some(Value::Str(s)) => {
            // Try hex first: 0x5
            if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                if let Ok(n) = i64::from_str_radix(hex, 16) {
                    return Ok(Value::Int(n));
                }
            }
            if let Ok(n) = s.parse::<i64>() {
                Ok(Value::Int(n))
            } else if let Ok(f) = s.parse::<f64>() {
                Ok(Value::Float(f))
            } else {
                Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: format!("Can't parse string '{}' as number.", s),
                    actual: format!("'{}'", s),
                }))
            }
        }
        Some(Value::Bool(b)) => Ok(Value::Int(if *b { 1 } else { 0 })),
        Some(Value::DateTime(dt)) => {
            // DateTime → epoch millis
            Ok(Value::Int(dt.timestamp().as_millisecond()))
        }
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "Can't parse type ArrayList as number.".into(),
            actual: "list".into(),
        })),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn asnumber_typed(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // asNumber(GType.INT/LONG/FLOAT/DOUBLE/BYTE/SHORT): convert to specific numeric type
        match (vals.first(), vals.get(1)) {
            (_, Some(Value::Str(t))) if t.as_ref() == "__invalid__" => {
                Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "asNumber() requires a numeric type token".into(),
                    actual: "non-numeric type".into(),
                }))
            }
            (Some(Value::Int(n)), Some(Value::Str(t))) if t.as_ref() == "byte" => {
                if *n < i8::MIN as i64 || *n > i8::MAX as i64 {
                    Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Can't convert number of type Integer to Byte due to overflow."
                            .into(),
                        actual: n.to_string(),
                    }))
                } else {
                    Ok(Value::Int(*n))
                }
            }
            (Some(Value::Int(n)), Some(Value::Str(t))) if t.as_ref() == "short" => {
                if *n < i16::MIN as i64 || *n > i16::MAX as i64 {
                    Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Can't convert number of type Integer to Short due to overflow."
                            .into(),
                        actual: n.to_string(),
                    }))
                } else {
                    Ok(Value::Int(*n))
                }
            }
            (Some(Value::Float(f)), Some(Value::Str(t))) if t.as_ref() == "int" => {
                Ok(Value::Int(*f as i64))
            }
            (Some(Value::Int(n)), Some(Value::Str(t))) if t.as_ref() == "float" => {
                Ok(Value::Float(*n as f64))
            }
            (Some(Value::Str(s)), Some(Value::Str(t))) if t.as_ref() == "int" => {
                if let Ok(n) = s.parse::<i64>() {
                    Ok(Value::Int(n))
                } else if let Ok(f) = s.parse::<f64>() {
                    Ok(Value::Int(f as i64))
                } else {
                    Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: format!("Can't parse string '{}' as integer.", s),
                        actual: format!("'{}'", s),
                    }))
                }
            }
            (Some(Value::Str(s)), Some(Value::Str(t))) if t.as_ref() == "float" => {
                if let Ok(f) = s.parse::<f64>() {
                    Ok(Value::Float(f))
                } else {
                    Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: format!("Can't parse string '{}' as float.", s),
                        actual: format!("'{}'", s),
                    }))
                }
            }
            (Some(Value::Str(s)), Some(Value::Str(t))) if t.as_ref() == "byte" => {
                if let Ok(n) = s.parse::<i64>() {
                    if n < i8::MIN as i64 || n > i8::MAX as i64 {
                        Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected:
                                "Can't convert number of type Integer to Byte due to overflow."
                                    .into(),
                            actual: n.to_string(),
                        }))
                    } else {
                        Ok(Value::Int(n))
                    }
                } else {
                    Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: format!("Can't parse string '{}' as byte.", s),
                        actual: format!("'{}'", s),
                    }))
                }
            }
            (Some(Value::Str(s)), Some(Value::Str(t))) if t.as_ref() == "short" => {
                if let Ok(n) = s.parse::<i64>() {
                    if n < i16::MIN as i64 || n > i16::MAX as i64 {
                        Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected:
                                "Can't convert number of type Integer to Short due to overflow."
                                    .into(),
                            actual: n.to_string(),
                        }))
                    } else {
                        Ok(Value::Int(n))
                    }
                } else {
                    Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: format!("Can't parse string '{}' as short.", s),
                        actual: format!("'{}'", s),
                    }))
                }
            }
            // byte/short for float inputs — truncate if in range
            (Some(Value::Float(f)), Some(Value::Str(t))) if t.as_ref() == "byte" => {
                let n = *f as i64;
                if n < i8::MIN as i64 || n > i8::MAX as i64 {
                    Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Can't convert number to Byte due to overflow.".into(),
                        actual: f.to_string(),
                    }))
                } else {
                    Ok(Value::Int(n))
                }
            }
            (Some(Value::Float(f)), Some(Value::Str(t))) if t.as_ref() == "short" => {
                let n = *f as i64;
                if n < i16::MIN as i64 || n > i16::MAX as i64 {
                    Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Can't convert number to Short due to overflow.".into(),
                        actual: f.to_string(),
                    }))
                } else {
                    Ok(Value::Int(n))
                }
            }
            (Some(val), _) => {
                // Fall through to default asNumber behavior
                match val {
                    Value::Int(n) => Ok(Value::Int(*n)),
                    Value::Float(f) => Ok(Value::Float(*f)),
                    Value::Null => Ok(Value::Null),
                    _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Cannot convert to number".into(),
                        actual: val.type_name().to_string(),
                    })),
                }
            }
            _ => Ok(Value::Null),
        }
    }
}

pub(crate) fn asbool(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Bool(b)) => Ok(Value::Bool(*b)),
        Some(Value::Int(n)) => Ok(Value::Bool(*n != 0)),
        Some(Value::Float(f)) => {
            if f.is_nan() {
                Ok(Value::Bool(false))
            } else {
                Ok(Value::Bool(*f != 0.0))
            }
        }
        Some(Value::Str(s)) => match s.to_lowercase().as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "boolean-compatible value".into(),
                actual: format!("'{}'", s),
            })),
        },
        Some(Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "boolean-compatible value".into(),
            actual: "null".into(),
        })),
        Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "boolean-compatible value".into(),
            actual: "list".into(),
        })),
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "boolean-compatible value".into(),
            actual: "unknown".into(),
        })),
    }
}

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
