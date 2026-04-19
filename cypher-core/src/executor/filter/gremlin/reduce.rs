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
    fn fold_reduce_sum_int_and_float() {
        let r = fold_reduce(vec![
            l(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            Value::Int(10),
            s("sum"),
        ])
        .unwrap();
        assert_eq!(r, Value::Int(16));
        let r = fold_reduce(vec![
            l(vec![Value::Float(1.5), Value::Int(2)]),
            Value::Float(0.5),
            s("sum"),
        ])
        .unwrap();
        assert_eq!(r, Value::Float(4.0));
        // unknown op returns seed
        assert_eq!(
            fold_reduce(vec![l(vec![]), Value::Int(7), s("unknown")]).unwrap(),
            Value::Int(7)
        );
        // default op when missing
        assert_eq!(
            fold_reduce(vec![l(vec![Value::Int(1)]), Value::Int(0)]).unwrap(),
            Value::Int(1)
        );
    }

    #[test]
    fn fold_reduce_addall_maps() {
        let mut a = IndexMap::<Box<str>, Value>::new();
        a.insert("k".into(), Value::Int(1));
        let mut b = IndexMap::<Box<str>, Value>::new();
        b.insert("k".into(), Value::Int(99));
        b.insert("j".into(), Value::Int(2));
        let r = fold_reduce(vec![l(vec![Value::Map(b)]), Value::Map(a), s("addall")]).unwrap();
        if let Value::Map(m) = r {
            assert_eq!(m.get("k"), Some(&Value::Int(99)));
            assert_eq!(m.get("j"), Some(&Value::Int(2)));
        }
        // addall with non-map seed → starts empty
        let mut b = IndexMap::<Box<str>, Value>::new();
        b.insert("x".into(), Value::Int(5));
        let r = fold_reduce(vec![l(vec![Value::Map(b)]), Value::Int(0), s("addall")]).unwrap();
        if let Value::Map(m) = r {
            assert_eq!(m.get("x"), Some(&Value::Int(5)));
        }
    }

    #[test]
    fn math_basic_and_extra_vars() {
        let r = math(vec![Value::Int(3), s("_ + 1")]).unwrap();
        assert_eq!(r, Value::Float(4.0));
        let r = math(vec![Value::Int(2), s("_ * x"), s("x"), Value::Int(5)]).unwrap();
        assert_eq!(r, Value::Float(10.0));
        // missing expr -> Null
        assert_eq!(math(vec![Value::Int(1)]).unwrap(), Value::Null);
    }

    #[test]
    fn tail_local_list_and_map_and_other() {
        let r = tail_local(vec![
            l(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            Value::Int(2),
        ])
        .unwrap();
        assert_eq!(r, l(vec![Value::Int(2), Value::Int(3)]));
        // Default n=1
        let r = tail_local(vec![l(vec![Value::Int(1), Value::Int(2)])]).unwrap();
        assert_eq!(r, l(vec![Value::Int(2)]));
        // n bigger than length
        let r = tail_local(vec![l(vec![Value::Int(1)]), Value::Int(5)]).unwrap();
        assert_eq!(r, l(vec![Value::Int(1)]));
        // Map
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("a".into(), Value::Int(1));
        m.insert("b".into(), Value::Int(2));
        m.insert("c".into(), Value::Int(3));
        let r = tail_local(vec![Value::Map(m), Value::Int(2)]).unwrap();
        if let Value::Map(m) = r {
            assert_eq!(m.len(), 2);
            assert!(m.contains_key("b"));
            assert!(m.contains_key("c"));
        }
        // Null and scalar
        assert_eq!(tail_local(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(tail_local(vec![Value::Int(7)]).unwrap(), Value::Int(7));
    }

    #[test]
    fn dedup_local_simple_and_keyed() {
        let r = dedup_local(vec![l(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(1),
            Value::Int(3),
        ])])
        .unwrap();
        assert_eq!(r, l(vec![Value::Int(1), Value::Int(2), Value::Int(3)]));
        // Keyed dedup over list of maps
        let mut a = IndexMap::<Box<str>, Value>::new();
        a.insert("k".into(), Value::Int(1));
        let mut b = IndexMap::<Box<str>, Value>::new();
        b.insert("k".into(), Value::Int(2));
        let mut c = IndexMap::<Box<str>, Value>::new();
        c.insert("k".into(), Value::Int(1));
        let r = dedup_local(vec![
            l(vec![Value::Map(a), Value::Map(b), Value::Map(c)]),
            s("k"),
        ])
        .unwrap();
        if let Value::List(items) = r {
            assert_eq!(items.len(), 2);
        }
        // Null and scalar pass-through
        assert_eq!(dedup_local(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(dedup_local(vec![Value::Int(5)]).unwrap(), Value::Int(5));
    }
}
