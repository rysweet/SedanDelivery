//! Gremlin reduce functions extracted from filter::eval_function_with_vals.
//!
//! Behavior preserved verbatim from the original `filter` module.

#![allow(clippy::needless_pass_by_value)]
#![allow(unused_imports)]

use indexmap::IndexMap;

use super::super::math::eval_math_expr;
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn fold_reduce(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // fold(seed, Operator) — reduce a collected list with a binary operator.
        // Args: list, seed, operator_name
        let list = vals.first().cloned().unwrap_or(Value::Null);
        let seed = vals.get(1).cloned().unwrap_or(Value::Null);
        let op = match vals.get(2) {
            Some(Value::Str(s)) => s.as_ref(),
            _ => "sum",
        };
        let items = match list {
            Value::List(l) => l,
            _ => vec![],
        };
        match op {
            "sum" => {
                let mut acc = seed;
                for v in items {
                    acc = match (&acc, &v) {
                        (Value::Int(a), Value::Int(b)) => Value::Int(a + b),
                        (Value::Float(a), Value::Int(b)) => Value::Float(a + *b as f64),
                        (Value::Int(a), Value::Float(b)) => Value::Float(*a as f64 + b),
                        (Value::Float(a), Value::Float(b)) => Value::Float(a + b),
                        _ => acc,
                    };
                }
                Ok(acc)
            }
            "addall" => {
                // Merge maps: last-writer-wins
                let mut result = match seed {
                    Value::Map(m) => m,
                    _ => IndexMap::new(),
                };
                for v in items {
                    if let Value::Map(m) = v {
                        for (k, val) in m {
                            result.insert(k, val);
                        }
                    }
                }
                Ok(Value::Map(result))
            }
            _ => Ok(seed),
        }
    }
}

pub(crate) fn math(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        let current_val = vals.first().cloned().unwrap_or(Value::Null);
        let expr_str = match vals.get(1) {
            Some(Value::Str(s)) => s.as_ref(),
            _ => return Ok(Value::Null),
        };
        // Parse extra variable bindings: pairs of (name, value) starting at index 2
        let mut var_bindings: Vec<(String, f64)> = Vec::new();
        let mut i = 2;
        while i + 1 < vals.len() {
            if let (Some(Value::Str(name)), Some(val)) = (vals.get(i), vals.get(i + 1)) {
                let num = match val {
                    Value::Int(n) => *n as f64,
                    Value::Float(f) => *f,
                    _ => 0.0,
                };
                var_bindings.push((name.to_string(), num));
            }
            i += 2;
        }
        eval_math_expr(expr_str, &current_val, &var_bindings)
    }
}

pub(crate) fn tail_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        let val = vals.first().cloned().unwrap_or(Value::Null);
        let n = match vals.get(1) {
            Some(Value::Int(i)) => *i as usize,
            _ => 1,
        };
        match val {
            Value::List(items) => {
                let start = items.len().saturating_sub(n);
                Ok(Value::List(items[start..].to_vec()))
            }
            Value::Map(m) => {
                let entries: Vec<_> = m.iter().collect();
                let start = entries.len().saturating_sub(n);
                let result: IndexMap<Box<str>, Value> = entries[start..]
                    .iter()
                    .map(|(k, v)| ((*k).clone(), (*v).clone()))
                    .collect();
                Ok(Value::Map(result))
            }
            Value::Null => Ok(Value::Null),
            other => Ok(other),
        }
    }
}

pub(crate) fn dedup_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // dedup(Scope.local) — deduplicate elements within a list
        // Args: [list_value] or [list_value, key1, key2, ...]
        let val = vals.first().cloned().unwrap_or(Value::Null);
        match val {
            Value::List(items) => {
                if vals.len() > 1 {
                    // Keyed dedup: dedup(Scope.local, "x", "y")
                    // Items are Maps; dedup by the specified key values
                    let keys: Vec<&str> = vals[1..]
                        .iter()
                        .filter_map(|v| {
                            if let Value::Str(s) = v {
                                Some(s.as_ref())
                            } else {
                                None
                            }
                        })
                        .collect();
                    let mut seen = std::collections::HashSet::new();
                    let mut result = Vec::new();
                    for item in &items {
                        let dedup_key = if let Value::Map(m) = item {
                            keys.iter()
                                .map(|k| m.get(*k).map(|v| v.canonical_key()).unwrap_or_default())
                                .collect::<Vec<_>>()
                                .join("\x1f")
                        } else {
                            item.canonical_key()
                        };
                        if seen.insert(dedup_key) {
                            result.push(item.clone());
                        }
                    }
                    Ok(Value::List(result))
                } else {
                    // Simple dedup: dedup(Scope.local)
                    let mut seen = std::collections::HashSet::new();
                    let mut result = Vec::new();
                    for item in &items {
                        let key = item.canonical_key();
                        if seen.insert(key) {
                            result.push(item.clone());
                        }
                    }
                    Ok(Value::List(result))
                }
            }
            Value::Null => Ok(Value::Null),
            other => Ok(other),
        }
    }
}
