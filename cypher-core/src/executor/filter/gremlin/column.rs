//! Gremlin column functions extracted from filter::eval_function_with_vals.
//!
//! Behavior preserved verbatim from the original `filter` module.

#![allow(clippy::needless_pass_by_value)]
#![allow(unused_imports)]

use indexmap::IndexMap;

use super::super::gremlin_pred::extract_sort_key;
use super::super::map_key::map_key_to_value;
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn column_key(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Map(m)) if m.len() == 1 => Ok(map_key_to_value(m.keys().next().unwrap())),
        Some(other) => Ok(other.clone()),
        None => Ok(Value::Null),
    }
}

pub(crate) fn select_values(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Map(m)) if m.len() == 1 => {
            let v = m.values().next().cloned().unwrap_or(Value::Null);
            // Single-entry map: if the value is already a List (e.g.,
            // from valueMap), wrap in an outer list to get correct
            // select(values) semantics.  If scalar (e.g., unfolded
            // groupCount entry), return it directly.
            match v {
                Value::List(_) => Ok(Value::List(vec![v])),
                other => Ok(other),
            }
        }
        Some(Value::Map(m)) => Ok(Value::List(m.values().cloned().collect())),
        Some(Value::List(items)) => Ok(Value::List(items.clone())),
        other => Ok(other.cloned().unwrap_or(Value::Null)),
    }
}

pub(crate) fn column_value(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Map(m)) if m.len() == 1 => {
            Ok(m.values().next().cloned().unwrap_or(Value::Null))
        }
        Some(other) => Ok(other.clone()),
        None => Ok(Value::Null),
    }
}

pub(crate) fn column_values(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Map(m)) => Ok(Value::List(m.values().cloned().collect())),
        Some(Value::List(items)) => Ok(Value::List(items.clone())),
        other => Ok(other.cloned().unwrap_or(Value::Null)),
    }
}

pub(crate) fn column_keys(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Map(m)) => Ok(Value::List(
            m.keys().map(|k| Value::Str(k.clone())).collect(),
        )),
        Some(Value::List(items)) => {
            // For a list, keys are the indices
            Ok(Value::List(
                (0..items.len()).map(|i| Value::Int(i as i64)).collect(),
            ))
        }
        other => Ok(other.cloned().unwrap_or(Value::Null)),
    }
}

pub(crate) fn sort_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        let mut vals_iter = vals.into_iter();
        let list_val = vals_iter.next().unwrap_or(Value::Null);
        let by_key = match vals_iter.next() {
            Some(Value::Str(s)) if !s.is_empty() => Some(s),
            _ => None,
        };
        let descending = matches!(vals_iter.next(), Some(Value::Str(s)) if s.as_ref() == "desc");

        match list_val {
            Value::List(mut items) => {
                if let Some(ref key) = by_key {
                    // Sort by property key — extract key from each item for comparison
                    items.sort_by(|a, b| {
                        let a_prop = extract_sort_key(a, key);
                        let b_prop = extract_sort_key(b, key);
                        let ord =
                            crate::executor::engine::helpers::gremlin_cmp_values(&a_prop, &b_prop);
                        if descending {
                            ord.reverse()
                        } else {
                            ord
                        }
                    });
                } else {
                    items.sort_by(|a, b| {
                        let ord = crate::executor::engine::helpers::gremlin_cmp_values(a, b);
                        if descending {
                            ord.reverse()
                        } else {
                            ord
                        }
                    });
                }
                Ok(Value::List(items))
            }
            Value::Map(m) => {
                // Sort map entries by value (Column.values) or key (Column.keys)
                let mut entries: Vec<(Box<str>, Value)> = m.into_iter().collect();
                if let Some(ref key) = by_key {
                    if key.as_ref() == "Column.keys" {
                        // Sort by key name; strip t[...] wrapper for T tokens
                        // so t[id] compares as "id" and t[label] as "label"
                        entries.sort_by(|(ka, _), (kb, _)| {
                            let ka_cmp = ka
                                .strip_prefix("t[")
                                .and_then(|s| s.strip_suffix(']'))
                                .unwrap_or(ka);
                            let kb_cmp = kb
                                .strip_prefix("t[")
                                .and_then(|s| s.strip_suffix(']'))
                                .unwrap_or(kb);
                            ka_cmp.cmp(kb_cmp)
                        });
                    } else {
                        entries.sort_by(|(_, va), (_, vb)| {
                            crate::executor::engine::helpers::gremlin_cmp_values(va, vb)
                        });
                    }
                } else {
                    entries.sort_by(|(ka, _), (kb, _)| ka.cmp(kb));
                }
                if descending {
                    entries.reverse();
                }
                let result: indexmap::IndexMap<Box<str>, Value> = entries.into_iter().collect();
                Ok(Value::Map(result))
            }
            other => Ok(other),
        }
    }
}

pub(crate) fn sum_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::List(items)) => {
            if items.is_empty() {
                return Ok(Value::Null);
            }
            let mut sum_i: i64 = 0;
            let mut sum_f: f64 = 0.0;
            let mut is_float = false;
            let mut has_value = false;
            for v in items {
                match v {
                    Value::Int(n) => {
                        sum_i += n;
                        has_value = true;
                    }
                    Value::Float(f) => {
                        sum_f += f;
                        is_float = true;
                        has_value = true;
                    }
                    Value::Null => {} // skip nulls
                    _ => {}
                }
            }
            if !has_value {
                Ok(Value::Null)
            } else if is_float {
                Ok(Value::Float(sum_f + sum_i as f64))
            } else {
                Ok(Value::Int(sum_i))
            }
        }
        Some(Value::Int(n)) => Ok(Value::Int(*n)),
        Some(Value::Float(f)) => Ok(Value::Float(*f)),
        Some(Value::Null) => Ok(Value::Null),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn min_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::List(items)) => {
            let mut result: Option<Value> = None;
            for v in items {
                if matches!(v, Value::Null) {
                    continue;
                }
                result = Some(match result {
                    None => v.clone(),
                    Some(cur) => {
                        if let Ok(std::cmp::Ordering::Less) = super::super::cypher_compare(v, &cur)
                        {
                            v.clone()
                        } else {
                            cur
                        }
                    }
                });
            }
            Ok(result.unwrap_or(Value::Null))
        }
        Some(Value::Int(n)) => Ok(Value::Int(*n)),
        Some(Value::Float(f)) => Ok(Value::Float(*f)),
        Some(Value::Str(s)) => Ok(Value::Str(s.clone())),
        Some(Value::Null) => Ok(Value::Null),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn max_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::List(items)) => {
            let mut result: Option<Value> = None;
            for v in items {
                if matches!(v, Value::Null) {
                    continue;
                }
                result = Some(match result {
                    None => v.clone(),
                    Some(cur) => {
                        if let Ok(std::cmp::Ordering::Greater) =
                            super::super::cypher_compare(v, &cur)
                        {
                            v.clone()
                        } else {
                            cur
                        }
                    }
                });
            }
            Ok(result.unwrap_or(Value::Null))
        }
        Some(Value::Int(n)) => Ok(Value::Int(*n)),
        Some(Value::Float(f)) => Ok(Value::Float(*f)),
        Some(Value::Str(s)) => Ok(Value::Str(s.clone())),
        Some(Value::Null) => Ok(Value::Null),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn mean_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::List(items)) => {
            let mut sum: f64 = 0.0;
            let mut count: usize = 0;
            for v in items {
                match v {
                    Value::Int(n) => {
                        sum += *n as f64;
                        count += 1;
                    }
                    Value::Float(f) => {
                        sum += f;
                        count += 1;
                    }
                    _ => {}
                }
            }
            if count == 0 {
                Ok(Value::Null)
            } else {
                Ok(Value::Float(sum / count as f64))
            }
        }
        Some(Value::Int(n)) => Ok(Value::Float(*n as f64)),
        Some(Value::Float(f)) => Ok(Value::Float(*f)),
        Some(Value::Null) => Ok(Value::Null),
        _ => Ok(Value::Null),
    }
}
