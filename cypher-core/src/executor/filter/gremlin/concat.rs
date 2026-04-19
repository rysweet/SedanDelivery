//! Gremlin concat functions extracted from filter::eval_function_with_vals.
//!
//! Behavior preserved verbatim from the original `filter` module.

#![allow(clippy::needless_pass_by_value)]
#![allow(unused_imports)]

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn concat(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // Gremlin concat: first arg is current traverser, rest are args to append.
        // Null traverser (first arg) with no other args → null.
        // Null args → skipped.
        // List as traverser (first arg) → error.
        if vals.is_empty() {
            return Ok(Value::Null);
        }
        if matches!(vals.first(), Some(Value::List(_))) {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "String concat() can only take string as argument".into(),
                actual: "list".into(),
            }));
        }
        // If only arg is null traverser and no concat args, return null
        if vals.len() == 1 && matches!(vals.first(), Some(Value::Null)) {
            return Ok(Value::Null);
        }
        let mut result = String::new();
        for v in vals.iter() {
            match v {
                Value::Str(s) => result.push_str(s),
                Value::Int(n) => result.push_str(&n.to_string()),
                Value::Float(f) => result.push_str(&f.to_string()),
                Value::Bool(b) => result.push_str(&b.to_string()),
                Value::Null => {
                    // Null args are skipped
                }
                _ => result.push_str(&format!("{:?}", v)),
            }
        }
        Ok(Value::Str(result.into()))
    }
}

pub(crate) fn conjoin(vals: Vec<Value>) -> Result<Value, GqlError> {
    match (vals.first(), vals.get(1)) {
            (Some(Value::List(items)), Some(Value::Str(sep))) => {
                let parts: Vec<String> = items
                    .iter()
                    .filter_map(|v| match v {
                        Value::Str(s) => Some(s.to_string()),
                        Value::Int(n) => Some(n.to_string()),
                        Value::Float(f) => Some(f.to_string()),
                        Value::Bool(b) => Some(b.to_string()),
                        Value::Null => None, // skip null elements
                        other => Some(format!("{:?}", other)),
                    })
                    .collect();
                Ok(Value::Str(parts.join(sep.as_ref()).into()))
            }
            (Some(Value::Null), _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Incoming traverser for conjoin step can't be null".into(),
                actual: "null".into(),
            })),
            (_, Some(Value::Str(_))) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "conjoin step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                actual: "non-list".into(),
            })),
            _ => Ok(Value::Null),
        }
}

pub(crate) fn format(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // args: [traverser, pattern, optional_labels_map]
        match (vals.first(), vals.get(1)) {
            (Some(val), Some(Value::Str(pattern))) => {
                let mut result = pattern.to_string();
                // First, resolve %{label} from optional labels map (3rd arg)
                if let Some(Value::Map(labels)) = vals.get(2) {
                    for (k, v) in labels.iter() {
                        let placeholder = format!("%{{{}}}", k);
                        if result.contains(&placeholder) {
                            let val_str = match v {
                                Value::Str(s) => s.to_string(),
                                Value::Int(n) => n.to_string(),
                                Value::Float(f) => f.to_string(),
                                Value::Bool(b) => b.to_string(),
                                _ => format!("{:?}", v),
                            };
                            result = result.replace(&placeholder, &val_str);
                        }
                    }
                }
                // Then resolve %{key} from the traverser value if it's a Map
                if result.contains("%{") {
                    if let Value::Map(m) = val {
                        for (k, v) in m.iter() {
                            let placeholder = format!("%{{{}}}", k);
                            let val_str = match v {
                                Value::Str(s) => s.to_string(),
                                Value::Int(n) => n.to_string(),
                                Value::Float(f) => f.to_string(),
                                Value::Bool(b) => b.to_string(),
                                _ => format!("{:?}", v),
                            };
                            result = result.replace(&placeholder, &val_str);
                        }
                    }
                }
                // If any %{...} placeholders remain unresolved, return Null
                // so the filter can drop this traverser.
                if result.contains("%{") {
                    return Ok(Value::Null);
                }
                // Replace %s with the traverser value directly
                if result.contains("%s") {
                    let val_str = match val {
                        Value::Str(s) => s.to_string(),
                        Value::Int(n) => n.to_string(),
                        Value::Float(f) => f.to_string(),
                        _ => String::new(),
                    };
                    result = result.replace("%s", &val_str);
                }
                Ok(Value::Str(result.into()))
            }
            (Some(Value::Null), _) => Ok(Value::Null),
            _ => Ok(Value::Null),
        }
    }
}

pub(crate) fn format_by(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // format() with by() modulations: args = [traverser, pattern, by1, by2, ...]
        // Replace %{_} placeholders cyclically with by-values.
        // Also resolve %{name} from node/edge properties.
        if vals.len() < 2 {
            return Ok(Value::Null);
        }
        if matches!(vals.first(), Some(Value::Null)) {
            return Ok(Value::Null);
        }
        let pattern = match vals.get(1) {
            Some(Value::Str(s)) => s.to_string(),
            _ => return Ok(Value::Null),
        };
        let by_values = &vals[2..];
        let mut result = pattern;
        // Replace %{_} with by-values cyclically
        let mut by_idx = 0;
        while let Some(pos) = result.find("%{_}") {
            if by_values.is_empty() {
                break;
            }
            let val = &by_values[by_idx % by_values.len()];
            let val_str = match val {
                Value::Str(s) => s.to_string(),
                Value::Int(n) => n.to_string(),
                Value::Float(f) => f.to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Null => return Ok(Value::Null),
                _ => format!("{:?}", val),
            };
            result = format!("{}{}{}", &result[..pos], val_str, &result[pos + 4..]);
            by_idx += 1;
        }
        // If there are still %{key} placeholders, try resolving from node/edge properties
        if result.contains("%{") {
            return Ok(Value::Null);
        }
        Ok(Value::Str(result.into()))
    }
}

pub(crate) fn make_datetime(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // Parse an ISO datetime string into Value::DateTime
        match vals.first() {
            Some(Value::Str(s)) => {
                if let Ok(ts) = s.as_ref().parse::<jiff::Timestamp>() {
                    let tz = jiff::tz::TimeZone::UTC;
                    Ok(Value::DateTime(Box::new(ts.to_zoned(tz))))
                } else {
                    Ok(Value::Str(s.clone()))
                }
            }
            _ => Ok(Value::Null),
        }
    }
}

pub(crate) fn make_uuid(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // Wrap a UUID string with a recognizable tag for ordering
        match vals.first() {
            Some(Value::Str(s)) => Ok(Value::Str(format!("~uuid:{}", s).into())),
            _ => Ok(Value::Null),
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
    fn concat_scalars_and_skip_null() {
        assert_eq!(concat(vec![s("a"), s("b")]).unwrap(), s("ab"));
        assert_eq!(
            concat(vec![
                Value::Int(1),
                s("-"),
                Value::Float(2.5),
                Value::Bool(true)
            ])
            .unwrap(),
            s("1-2.5true")
        );
        // Null-only single arg
        assert_eq!(concat(vec![Value::Null]).unwrap(), Value::Null);
        // Null arg with others is skipped
        assert_eq!(concat(vec![s("x"), Value::Null, s("y")]).unwrap(), s("xy"));
        // Empty
        assert_eq!(concat(vec![]).unwrap(), Value::Null);
        // List as traverser → error
        assert!(concat(vec![Value::List(vec![])]).is_err());
    }

    #[test]
    fn conjoin_basic_and_errors() {
        assert_eq!(
            conjoin(vec![
                Value::List(vec![s("a"), s("b"), Value::Int(1), Value::Null]),
                s(", "),
            ])
            .unwrap(),
            s("a, b, 1")
        );
        assert!(conjoin(vec![Value::Null, s(",")]).is_err());
        assert!(conjoin(vec![Value::Int(1), s(",")]).is_err());
        assert_eq!(conjoin(vec![Value::Int(1)]).unwrap(), Value::Null);
    }

    #[test]
    fn format_resolves_placeholders() {
        let mut labels = IndexMap::<Box<str>, Value>::new();
        labels.insert("env".into(), s("prod"));
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("name".into(), s("alice"));
        // %{env} from labels, %{name} from traverser map
        let r = format(vec![
            Value::Map(m),
            s("hi %{name} on %{env}"),
            Value::Map(labels),
        ])
        .unwrap();
        assert_eq!(r, s("hi alice on prod"));
        // Unresolved %{...} -> Null
        let r = format(vec![Value::Int(1), s("%{x}")]).unwrap();
        assert_eq!(r, Value::Null);
        // %s with scalar
        assert_eq!(format(vec![Value::Int(7), s("v=%s")]).unwrap(), s("v=7"));
        assert_eq!(format(vec![Value::Null, s("any")]).unwrap(), s("any"));
        assert_eq!(format(vec![]).unwrap(), Value::Null);
    }

    #[test]
    fn format_by_cycles_through_args() {
        assert_eq!(
            format_by(vec![Value::Int(0), s("[%{_}-%{_}]"), s("a"), s("b")]).unwrap(),
            s("[a-b]")
        );
        // Null traverser
        assert_eq!(
            format_by(vec![Value::Null, s("anything")]).unwrap(),
            Value::Null
        );
        // Too few args
        assert_eq!(format_by(vec![Value::Int(1)]).unwrap(), Value::Null);
        // Pattern not a string
        assert_eq!(
            format_by(vec![Value::Int(1), Value::Int(2)]).unwrap(),
            Value::Null
        );
        // Null in by-args yields Null
        assert_eq!(
            format_by(vec![Value::Int(0), s("%{_}"), Value::Null]).unwrap(),
            Value::Null
        );
        // Unresolved %{name} → Null
        assert_eq!(
            format_by(vec![Value::Int(0), s("%{name}"), s("a")]).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn make_datetime_and_uuid() {
        let r = make_datetime(vec![s("2020-01-02T03:04:05Z")]).unwrap();
        assert!(matches!(r, Value::DateTime(_)));
        // Unparseable → original string back
        assert_eq!(make_datetime(vec![s("nope")]).unwrap(), s("nope"));
        assert_eq!(make_datetime(vec![Value::Int(1)]).unwrap(), Value::Null);

        assert_eq!(make_uuid(vec![s("xyz")]).unwrap(), s("~uuid:xyz"));
        assert_eq!(make_uuid(vec![Value::Int(1)]).unwrap(), Value::Null);
    }
}
