//! `asString` / `asString(Scope.local)` conversions.

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

pub(crate) fn asstring_local(vals: Vec<Value>) -> Result<Value, GqlError> {
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
            let converted: Result<Vec<Value>, GqlError> = items.iter().map(value_to_str).collect();
            Ok(Value::List(converted?))
        }
        Some(other) => value_to_str(other),
        None => Ok(Value::Null),
    }
}
