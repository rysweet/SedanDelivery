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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::Str(v.into())
    }

    fn l(items: Vec<Value>) -> Value {
        Value::List(items)
    }

    #[test]
    fn column_key_value_keys_values() {
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("k".into(), Value::Int(1));
        // single-entry map
        assert_eq!(column_key(vec![Value::Map(m.clone())]).unwrap(), s("k"));
        assert_eq!(
            column_value(vec![Value::Map(m.clone())]).unwrap(),
            Value::Int(1)
        );
        // pass-through and empty
        assert_eq!(column_key(vec![Value::Int(7)]).unwrap(), Value::Int(7));
        assert_eq!(column_key(vec![]).unwrap(), Value::Null);
        assert_eq!(column_value(vec![Value::Int(7)]).unwrap(), Value::Int(7));
        assert_eq!(column_value(vec![]).unwrap(), Value::Null);

        // multi-entry map for keys/values
        let mut m2 = IndexMap::<Box<str>, Value>::new();
        m2.insert("a".into(), Value::Int(1));
        m2.insert("b".into(), Value::Int(2));
        assert_eq!(
            column_keys(vec![Value::Map(m2.clone())]).unwrap(),
            l(vec![s("a"), s("b")])
        );
        assert_eq!(
            column_values(vec![Value::Map(m2.clone())]).unwrap(),
            l(vec![Value::Int(1), Value::Int(2)])
        );
        // List branches
        assert_eq!(
            column_keys(vec![l(vec![s("x"), s("y")])]).unwrap(),
            l(vec![Value::Int(0), Value::Int(1)])
        );
        assert_eq!(
            column_values(vec![l(vec![s("x"), s("y")])]).unwrap(),
            l(vec![s("x"), s("y")])
        );
        // Other / empty
        assert_eq!(column_keys(vec![Value::Int(7)]).unwrap(), Value::Int(7));
        assert_eq!(column_keys(vec![]).unwrap(), Value::Null);
        assert_eq!(column_values(vec![Value::Int(7)]).unwrap(), Value::Int(7));
        assert_eq!(column_values(vec![]).unwrap(), Value::Null);
    }

    #[test]
    fn select_values_branches() {
        // single-entry map with list value
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("k".into(), l(vec![Value::Int(1)]));
        assert_eq!(
            select_values(vec![Value::Map(m)]).unwrap(),
            l(vec![l(vec![Value::Int(1)])])
        );
        // single-entry map with scalar value
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("k".into(), Value::Int(7));
        assert_eq!(select_values(vec![Value::Map(m)]).unwrap(), Value::Int(7));
        // multi-entry map
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("a".into(), Value::Int(1));
        m.insert("b".into(), Value::Int(2));
        assert_eq!(
            select_values(vec![Value::Map(m)]).unwrap(),
            l(vec![Value::Int(1), Value::Int(2)])
        );
        // list passes through cloned
        assert_eq!(
            select_values(vec![l(vec![Value::Int(1)])]).unwrap(),
            l(vec![Value::Int(1)])
        );
        // scalar / empty
        assert_eq!(select_values(vec![Value::Int(5)]).unwrap(), Value::Int(5));
        assert_eq!(select_values(vec![]).unwrap(), Value::Null);
    }

    #[test]
    fn sort_local_list_and_map() {
        // Simple list ascending
        let r = sort_local(vec![l(vec![Value::Int(3), Value::Int(1), Value::Int(2)])]).unwrap();
        assert_eq!(r, l(vec![Value::Int(1), Value::Int(2), Value::Int(3)]));
        // Descending
        let r = sort_local(vec![
            l(vec![Value::Int(3), Value::Int(1), Value::Int(2)]),
            s(""),
            s("desc"),
        ])
        .unwrap();
        assert_eq!(r, l(vec![Value::Int(3), Value::Int(2), Value::Int(1)]));
        // Map sort by keys
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("b".into(), Value::Int(1));
        m.insert("a".into(), Value::Int(2));
        let r = sort_local(vec![Value::Map(m.clone())]).unwrap();
        if let Value::Map(mm) = r {
            let keys: Vec<&str> = mm.keys().map(|k| k.as_ref()).collect();
            assert_eq!(keys, vec!["a", "b"]);
        }
        // Map sort by Column.keys
        let r = sort_local(vec![Value::Map(m.clone()), s("Column.keys")]).unwrap();
        assert!(matches!(r, Value::Map(_)));
        // Map sort by other key (values)
        let r = sort_local(vec![Value::Map(m), s("v"), s("desc")]).unwrap();
        assert!(matches!(r, Value::Map(_)));
        // Pass-through scalar
        assert_eq!(sort_local(vec![Value::Int(7)]).unwrap(), Value::Int(7));
    }

    #[test]
    fn sum_min_max_mean_local() {
        // sum: ints, mixed, empty, null
        assert_eq!(
            sum_local(vec![l(vec![Value::Int(1), Value::Int(2), Value::Null])]).unwrap(),
            Value::Int(3)
        );
        assert_eq!(
            sum_local(vec![l(vec![Value::Int(1), Value::Float(0.5)])]).unwrap(),
            Value::Float(1.5)
        );
        assert_eq!(sum_local(vec![l(vec![])]).unwrap(), Value::Null);
        assert_eq!(sum_local(vec![Value::Int(7)]).unwrap(), Value::Int(7));
        assert_eq!(
            sum_local(vec![Value::Float(1.5)]).unwrap(),
            Value::Float(1.5)
        );
        assert_eq!(sum_local(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(sum_local(vec![s("x")]).unwrap(), Value::Null);

        // min/max: list with nulls + scalars
        assert_eq!(
            min_local(vec![l(vec![
                Value::Int(3),
                Value::Int(1),
                Value::Null,
                Value::Int(2)
            ])])
            .unwrap(),
            Value::Int(1)
        );
        assert_eq!(
            max_local(vec![l(vec![
                Value::Int(3),
                Value::Int(1),
                Value::Null,
                Value::Int(2)
            ])])
            .unwrap(),
            Value::Int(3)
        );
        assert_eq!(min_local(vec![l(vec![])]).unwrap(), Value::Null);
        assert_eq!(min_local(vec![Value::Int(5)]).unwrap(), Value::Int(5));
        assert_eq!(
            min_local(vec![Value::Float(1.5)]).unwrap(),
            Value::Float(1.5)
        );
        assert_eq!(min_local(vec![s("a")]).unwrap(), s("a"));
        assert_eq!(min_local(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(min_local(vec![Value::Bool(true)]).unwrap(), Value::Null);
        assert_eq!(max_local(vec![Value::Int(5)]).unwrap(), Value::Int(5));
        assert_eq!(
            max_local(vec![Value::Float(1.5)]).unwrap(),
            Value::Float(1.5)
        );
        assert_eq!(max_local(vec![s("a")]).unwrap(), s("a"));
        assert_eq!(max_local(vec![Value::Null]).unwrap(), Value::Null);

        // mean
        assert_eq!(
            mean_local(vec![l(vec![Value::Int(2), Value::Int(4)])]).unwrap(),
            Value::Float(3.0)
        );
        assert_eq!(
            mean_local(vec![l(vec![Value::Float(1.0), Value::Float(3.0)])]).unwrap(),
            Value::Float(2.0)
        );
        assert_eq!(mean_local(vec![l(vec![])]).unwrap(), Value::Null);
        assert_eq!(mean_local(vec![Value::Int(5)]).unwrap(), Value::Float(5.0));
        assert_eq!(
            mean_local(vec![Value::Float(1.5)]).unwrap(),
            Value::Float(1.5)
        );
        assert_eq!(mean_local(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(mean_local(vec![s("a")]).unwrap(), Value::Null);
    }
}
