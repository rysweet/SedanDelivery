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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{EdgeId, NodeId};

    fn s(v: &str) -> Value {
        Value::Str(v.into())
    }

    #[test]
    fn asstring_scalars_and_specials() {
        assert_eq!(asstring(vec![s("hi")]).unwrap(), s("hi"));
        assert_eq!(asstring(vec![Value::Int(42)]).unwrap(), s("42"));
        assert_eq!(asstring(vec![Value::Float(1.5)]).unwrap(), s("1.5"));
        assert_eq!(asstring(vec![Value::Float(f64::NAN)]).unwrap(), s("NaN"));
        assert_eq!(
            asstring(vec![Value::Float(f64::INFINITY)]).unwrap(),
            s("Infinity")
        );
        assert_eq!(
            asstring(vec![Value::Float(f64::NEG_INFINITY)]).unwrap(),
            s("-Infinity")
        );
        assert_eq!(asstring(vec![Value::Bool(true)]).unwrap(), s("true"));
        assert!(asstring(vec![Value::Null]).is_err());
        assert_eq!(asstring(vec![]).unwrap(), Value::Null);
        assert_eq!(asstring(vec![Value::Node(NodeId(7))]).unwrap(), s("v[7]"));
        assert_eq!(asstring(vec![Value::Edge(EdgeId(9))]).unwrap(), s("e[9]"));
    }

    #[test]
    fn asstring_list_and_map_and_vertex_property() {
        let r = asstring(vec![Value::List(vec![
            s("a"),
            Value::Int(1),
            Value::Float(2.5),
            Value::Bool(false),
            Value::Null,
        ])])
        .unwrap();
        assert_eq!(r, s("[a, 1, 2.5, false, null]"));
        // Vertex-property map
        let mut vp = IndexMap::<Box<str>, Value>::new();
        vp.insert("~key".into(), s("name"));
        vp.insert("~value".into(), s("alice"));
        assert_eq!(
            asstring(vec![Value::Map(vp)]).unwrap(),
            s("vp[name->alice]")
        );
        // Regular map with mixed types
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("a".into(), Value::Int(1));
        m.insert("b".into(), Value::List(vec![Value::Int(2), s("x")]));
        m.insert("~hidden".into(), s("skip"));
        let out = asstring(vec![Value::Map(m)]).unwrap();
        if let Value::Str(out) = out {
            assert!(out.contains("a=1"));
            assert!(out.contains("b=["));
            assert!(!out.contains("~hidden"));
        } else {
            panic!("expected str");
        }
    }

    #[test]
    fn asstring_path() {
        let v = Value::Path {
            nodes: vec![NodeId(1), NodeId(2)],
            edges: vec![EdgeId(10)],
        };
        assert_eq!(asstring(vec![v]).unwrap(), s("path[v[1], v[2], e[10]]"));
    }

    #[test]
    fn asdate_variants() {
        let r = asdate(vec![Value::Int(1700000000000)]).unwrap();
        assert!(matches!(r, Value::DateTime(_)));
        let r = asdate(vec![Value::Float(1700000000000.0)]).unwrap();
        assert!(matches!(r, Value::DateTime(_)));
        assert_eq!(asdate(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(asdate(vec![s("nope")]).unwrap(), Value::Null);
        // round-trip DateTime
        if let Value::DateTime(d) = asdate(vec![Value::Int(1)]).unwrap() {
            let r2 = asdate(vec![Value::DateTime(d.clone())]).unwrap();
            assert!(matches!(r2, Value::DateTime(_)));
        }
    }

    #[test]
    fn asstring_local_variants() {
        // List
        let r = asstring_local(vec![Value::List(vec![
            s("a"),
            Value::Int(1),
            Value::Bool(true),
        ])])
        .unwrap();
        assert_eq!(r, Value::List(vec![s("a"), s("1"), s("true")]));
        // Scalar
        assert_eq!(asstring_local(vec![Value::Int(7)]).unwrap(), s("7"));
        // Vertex-property map
        let mut vp = IndexMap::<Box<str>, Value>::new();
        vp.insert("~key".into(), s("k"));
        vp.insert("~value".into(), Value::Int(5));
        assert_eq!(asstring_local(vec![Value::Map(vp)]).unwrap(), s("vp[k->5]"));
        // Empty input
        assert_eq!(asstring_local(vec![]).unwrap(), Value::Null);
        // Null inside list propagates error
        assert!(asstring_local(vec![Value::List(vec![Value::Null])]).is_err());
    }

    #[test]
    fn asnumber_basics() {
        assert_eq!(asnumber(vec![Value::Int(5)]).unwrap(), Value::Int(5));
        assert_eq!(
            asnumber(vec![Value::Float(2.5)]).unwrap(),
            Value::Float(2.5)
        );
        assert_eq!(asnumber(vec![s("42")]).unwrap(), Value::Int(42));
        assert_eq!(asnumber(vec![s("0xff")]).unwrap(), Value::Int(255));
        assert_eq!(asnumber(vec![s("0X10")]).unwrap(), Value::Int(16));
        assert_eq!(asnumber(vec![s("2.5")]).unwrap(), Value::Float(2.5));
        assert!(asnumber(vec![s("foo")]).is_err());
        assert_eq!(asnumber(vec![Value::Bool(true)]).unwrap(), Value::Int(1));
        assert_eq!(asnumber(vec![Value::Bool(false)]).unwrap(), Value::Int(0));
        assert_eq!(asnumber(vec![Value::Null]).unwrap(), Value::Null);
        assert!(asnumber(vec![Value::List(vec![])]).is_err());
        assert_eq!(asnumber(vec![]).unwrap(), Value::Null);
    }

    #[test]
    fn asnumber_typed_combinations() {
        // byte ok and overflow
        assert_eq!(
            asnumber_typed(vec![Value::Int(5), s("byte")]).unwrap(),
            Value::Int(5)
        );
        assert!(asnumber_typed(vec![Value::Int(1000), s("byte")]).is_err());
        // short ok and overflow
        assert_eq!(
            asnumber_typed(vec![Value::Int(5), s("short")]).unwrap(),
            Value::Int(5)
        );
        assert!(asnumber_typed(vec![Value::Int(40000), s("short")]).is_err());
        // float -> int
        assert_eq!(
            asnumber_typed(vec![Value::Float(2.7), s("int")]).unwrap(),
            Value::Int(2)
        );
        // int -> float
        assert_eq!(
            asnumber_typed(vec![Value::Int(3), s("float")]).unwrap(),
            Value::Float(3.0)
        );
        // string -> int / float
        assert_eq!(
            asnumber_typed(vec![s("42"), s("int")]).unwrap(),
            Value::Int(42)
        );
        assert_eq!(
            asnumber_typed(vec![s("2.5"), s("int")]).unwrap(),
            Value::Int(2)
        );
        assert!(asnumber_typed(vec![s("nope"), s("int")]).is_err());
        assert_eq!(
            asnumber_typed(vec![s("2.5"), s("float")]).unwrap(),
            Value::Float(2.5)
        );
        assert!(asnumber_typed(vec![s("nope"), s("float")]).is_err());
        // string -> byte / short
        assert_eq!(
            asnumber_typed(vec![s("5"), s("byte")]).unwrap(),
            Value::Int(5)
        );
        assert!(asnumber_typed(vec![s("9999"), s("byte")]).is_err());
        assert!(asnumber_typed(vec![s("nan"), s("byte")]).is_err());
        assert_eq!(
            asnumber_typed(vec![s("5"), s("short")]).unwrap(),
            Value::Int(5)
        );
        assert!(asnumber_typed(vec![s("999999"), s("short")]).is_err());
        assert!(asnumber_typed(vec![s("nope"), s("short")]).is_err());
        // float -> byte / short including overflow
        assert_eq!(
            asnumber_typed(vec![Value::Float(5.0), s("byte")]).unwrap(),
            Value::Int(5)
        );
        assert!(asnumber_typed(vec![Value::Float(500.0), s("byte")]).is_err());
        assert_eq!(
            asnumber_typed(vec![Value::Float(5.0), s("short")]).unwrap(),
            Value::Int(5)
        );
        assert!(asnumber_typed(vec![Value::Float(50000.0), s("short")]).is_err());
        // invalid token branch
        assert!(asnumber_typed(vec![Value::Int(1), s("__invalid__")]).is_err());
        // fall-through for unknown type tag with int/float/null
        assert_eq!(
            asnumber_typed(vec![Value::Int(7), s("custom")]).unwrap(),
            Value::Int(7)
        );
        assert_eq!(
            asnumber_typed(vec![Value::Float(2.0), s("custom")]).unwrap(),
            Value::Float(2.0)
        );
        assert_eq!(
            asnumber_typed(vec![Value::Null, s("custom")]).unwrap(),
            Value::Null
        );
        assert!(asnumber_typed(vec![s("hi"), s("custom")]).is_err());
        // empty input
        assert_eq!(asnumber_typed(vec![]).unwrap(), Value::Null);
    }

    #[test]
    fn asbool_variants() {
        assert_eq!(asbool(vec![Value::Bool(true)]).unwrap(), Value::Bool(true));
        assert_eq!(asbool(vec![Value::Int(0)]).unwrap(), Value::Bool(false));
        assert_eq!(asbool(vec![Value::Int(1)]).unwrap(), Value::Bool(true));
        assert_eq!(asbool(vec![Value::Float(0.0)]).unwrap(), Value::Bool(false));
        assert_eq!(asbool(vec![Value::Float(2.0)]).unwrap(), Value::Bool(true));
        assert_eq!(
            asbool(vec![Value::Float(f64::NAN)]).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(asbool(vec![s("true")]).unwrap(), Value::Bool(true));
        assert_eq!(asbool(vec![s("False")]).unwrap(), Value::Bool(false));
        assert!(asbool(vec![s("nope")]).is_err());
        assert!(asbool(vec![Value::Null]).is_err());
        assert!(asbool(vec![Value::List(vec![])]).is_err());
        assert!(asbool(vec![]).is_err());
    }

    #[test]
    fn type_of_variants() {
        assert_eq!(type_of(vec![Value::Int(0)]).unwrap(), s("integer"));
        assert_eq!(type_of(vec![Value::Float(0.0)]).unwrap(), s("double"));
        assert_eq!(type_of(vec![s("x")]).unwrap(), s("string"));
        assert_eq!(type_of(vec![Value::Bool(true)]).unwrap(), s("boolean"));
        assert_eq!(type_of(vec![Value::Null]).unwrap(), s("null"));
        assert_eq!(type_of(vec![Value::List(vec![])]).unwrap(), s("list"));
        assert_eq!(
            type_of(vec![Value::Map(IndexMap::new())]).unwrap(),
            s("map")
        );
        assert_eq!(type_of(vec![Value::Node(NodeId(1))]).unwrap(), s("vertex"));
        assert_eq!(type_of(vec![Value::Edge(EdgeId(2))]).unwrap(), s("edge"));
        let v = Value::Path {
            nodes: vec![NodeId(1)],
            edges: vec![],
        };
        assert_eq!(type_of(vec![v]).unwrap(), s("path"));
        assert_eq!(type_of(vec![]).unwrap(), s("unknown"));
    }
}
