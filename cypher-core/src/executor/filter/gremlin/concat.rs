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
