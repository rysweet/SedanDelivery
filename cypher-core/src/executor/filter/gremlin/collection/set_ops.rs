//! Set-style binary operations on lists (and maps for `merge`).

#![allow(clippy::needless_pass_by_value)]

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn combine(vals: Vec<Value>) -> Result<Value, GqlError> {
    let lhs = vals.first().cloned().unwrap_or(Value::Null);
    let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
    match (&lhs, &rhs) {
            (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Incoming traverser for combine step can't be null".into(),
                actual: "null".into(),
            })),
            (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "traversal argument for combine step must yield an iterable type, not null".into(),
                actual: "null".into(),
            })),
            (Value::List(l), Value::List(r)) => {
                let mut result = l.clone();
                result.extend(r.iter().cloned());
                Ok(Value::List(result))
            }
            (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "combine step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                actual: format!("{:?}", lhs),
            })),
            _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "traversal argument for combine step must yield an iterable type, encountered".into(),
                actual: format!("{:?}", rhs),
            })),
        }
}

pub(crate) fn merge(vals: Vec<Value>) -> Result<Value, GqlError> {
    let lhs = vals.first().cloned().unwrap_or(Value::Null);
    let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
    match (&lhs, &rhs) {
            (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Incoming traverser for merge step can't be null".into(),
                actual: "null".into(),
            })),
            (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "traversal argument for merge step must yield an iterable type, not null".into(),
                actual: "null".into(),
            })),
            (Value::List(l), Value::List(r)) => {
                // merge() is set-union: unique elements from both lists
                let mut result = l.clone();
                for item in r {
                    if !result.iter().any(|existing| existing == item) {
                        result.push(item.clone());
                    }
                }
                Ok(Value::List(result))
            }
            (Value::List(_), Value::Map(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "step type mismatch: expected argument to be Iterable but got Map".into(),
                actual: "Map".into(),
            })),
            (Value::Map(l), Value::Map(r)) => {
                // Map merge: right side wins on key collisions (TinkerPop semantics)
                let mut result = l.clone();
                for (k, v) in r.iter() {
                    result.insert(k.clone(), v.clone());
                }
                Ok(Value::Map(result))
            }
            (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "merge step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                actual: format!("{:?}", lhs),
            })),
            (_, Value::Map(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "merge step expected provided argument to evaluate to a Map, encountered".into(),
                actual: format!("{:?}", rhs),
            })),
            _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "traversal argument for merge step must yield an iterable type, encountered".into(),
                actual: format!("{:?}", rhs),
            })),
        }
}

pub(crate) fn intersect(vals: Vec<Value>) -> Result<Value, GqlError> {
    let lhs = vals.first().cloned().unwrap_or(Value::Null);
    let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
    match (&lhs, &rhs) {
            (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Incoming traverser for intersect step can't be null".into(),
                actual: "null".into(),
            })),
            (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "traversal argument for intersect step must yield an iterable type, not null".into(),
                actual: "null".into(),
            })),
            (Value::List(l), Value::List(r)) => {
                // Multiset intersection: use swap_remove for O(1) removal
                // instead of remove() which is O(N) due to element shifting.
                let mut result = Vec::new();
                let mut rhs_remaining: Vec<&Value> = r.iter().collect();
                for item in l {
                    if let Some(pos) = rhs_remaining.iter().position(|rv| *rv == item) {
                        result.push(item.clone());
                        rhs_remaining.swap_remove(pos);
                    }
                }
                Ok(Value::List(result))
            }
            (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "intersect step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                actual: format!("{:?}", lhs),
            })),
            _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "traversal argument for intersect step must yield an iterable type, encountered".into(),
                actual: format!("{:?}", rhs),
            })),
        }
}

pub(crate) fn difference(vals: Vec<Value>) -> Result<Value, GqlError> {
    let lhs = vals.first().cloned().unwrap_or(Value::Null);
    let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
    match (&lhs, &rhs) {
        (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "Incoming traverser for difference step can't be null".into(),
            actual: "null".into(),
        })),
        (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected:
                "traversal argument for difference step must yield an iterable type, not null"
                    .into(),
            actual: "null".into(),
        })),
        (Value::List(l), Value::List(r)) => {
            // Multiset difference: use swap_remove for O(1) removal
            let mut result = Vec::new();
            let mut rhs_remaining: Vec<&Value> = r.iter().collect();
            for item in l {
                if let Some(pos) = rhs_remaining.iter().position(|rv| *rv == item) {
                    rhs_remaining.swap_remove(pos);
                } else {
                    result.push(item.clone());
                }
            }
            Ok(Value::List(result))
        }
        (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected:
                "difference step can only take an array or an Iterable type for incoming traversers"
                    .into(),
            actual: format!("{:?}", lhs),
        })),
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected:
                "traversal argument for difference step must yield an iterable type, encountered"
                    .into(),
            actual: format!("{:?}", rhs),
        })),
    }
}

pub(crate) fn disjunct(vals: Vec<Value>) -> Result<Value, GqlError> {
    let lhs = vals.first().cloned().unwrap_or(Value::Null);
    let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
    match (&lhs, &rhs) {
            (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Incoming traverser for disjunct step can't be null".into(),
                actual: "null".into(),
            })),
            (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "traversal argument for disjunct step must yield an iterable type, not null".into(),
                actual: "null".into(),
            })),
            (Value::List(l), Value::List(r)) => {
                // Symmetric difference: use swap_remove for O(1) removal
                let mut result = Vec::new();
                let mut r_remaining: Vec<&Value> = r.iter().collect();
                for item in l {
                    if let Some(pos) = r_remaining.iter().position(|rv| *rv == item) {
                        r_remaining.swap_remove(pos);
                    } else {
                        result.push(item.clone());
                    }
                }
                for item in r_remaining {
                    result.push(item.clone());
                }
                Ok(Value::List(result))
            }
            (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "disjunct step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                actual: format!("{:?}", lhs),
            })),
            _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "traversal argument for disjunct step must yield an iterable type, encountered".into(),
                actual: format!("{:?}", rhs),
            })),
        }
}

pub(crate) fn product(vals: Vec<Value>) -> Result<Value, GqlError> {
    let lhs = vals.first().cloned().unwrap_or(Value::Null);
    let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
    match (&lhs, &rhs) {
            (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Incoming traverser for product step can't be null".into(),
                actual: "null".into(),
            })),
            (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "traversal argument for product step must yield an iterable type, not null".into(),
                actual: "null".into(),
            })),
            (Value::List(l), Value::List(r)) => {
                if l.is_empty() || r.is_empty() {
                    return Ok(Value::List(Vec::new()));
                }
                let mut result = Vec::new();
                for a in l {
                    for b in r {
                        result.push(Value::List(vec![a.clone(), b.clone()]));
                    }
                }
                Ok(Value::List(result))
            }
            (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "product step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                actual: format!("{:?}", lhs),
            })),
            _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "traversal argument for product step must yield an iterable type, encountered".into(),
                actual: format!("{:?}", rhs),
            })),
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    fn l(items: Vec<Value>) -> Value {
        Value::List(items)
    }

    #[test]
    fn combine_lists_and_errors() {
        assert_eq!(
            combine(vec![l(vec![Value::Int(1)]), l(vec![Value::Int(2)])]).unwrap(),
            l(vec![Value::Int(1), Value::Int(2)])
        );
        assert!(combine(vec![Value::Null, l(vec![])]).is_err());
        assert!(combine(vec![l(vec![]), Value::Null]).is_err());
        assert!(combine(vec![Value::Int(1), l(vec![])]).is_err());
        assert!(combine(vec![l(vec![]), Value::Int(1)]).is_err());
    }

    #[test]
    fn merge_lists_and_maps_and_errors() {
        assert_eq!(
            merge(vec![
                l(vec![Value::Int(1)]),
                l(vec![Value::Int(1), Value::Int(2)])
            ])
            .unwrap(),
            l(vec![Value::Int(1), Value::Int(2)])
        );
        let mut a = IndexMap::<Box<str>, Value>::new();
        a.insert("k1".into(), Value::Int(1));
        a.insert("k2".into(), Value::Int(2));
        let mut b = IndexMap::<Box<str>, Value>::new();
        b.insert("k2".into(), Value::Int(99));
        b.insert("k3".into(), Value::Int(3));
        let r = merge(vec![Value::Map(a), Value::Map(b)]).unwrap();
        if let Value::Map(m) = r {
            assert_eq!(m.get("k2"), Some(&Value::Int(99)));
            assert_eq!(m.get("k3"), Some(&Value::Int(3)));
        } else {
            panic!("expected map");
        }
        // errors
        assert!(merge(vec![Value::Null, l(vec![])]).is_err());
        assert!(merge(vec![l(vec![]), Value::Null]).is_err());
        assert!(merge(vec![l(vec![]), Value::Map(IndexMap::new())]).is_err());
        assert!(merge(vec![Value::Int(1), l(vec![])]).is_err());
        assert!(merge(vec![Value::Int(1), Value::Map(IndexMap::new())]).is_err());
        assert!(merge(vec![Value::Int(1), Value::Int(2)]).is_err());
    }

    #[test]
    fn intersect_difference_disjunct() {
        assert_eq!(
            intersect(vec![
                l(vec![Value::Int(1), Value::Int(2), Value::Int(2)]),
                l(vec![Value::Int(2), Value::Int(3)]),
            ])
            .unwrap(),
            l(vec![Value::Int(2)])
        );
        assert!(intersect(vec![Value::Null, l(vec![])]).is_err());
        assert!(intersect(vec![l(vec![]), Value::Null]).is_err());
        assert!(intersect(vec![Value::Int(1), l(vec![])]).is_err());
        assert!(intersect(vec![Value::Int(1), Value::Int(2)]).is_err());

        assert_eq!(
            difference(vec![
                l(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
                l(vec![Value::Int(2)]),
            ])
            .unwrap(),
            l(vec![Value::Int(1), Value::Int(3)])
        );
        assert!(difference(vec![Value::Null, l(vec![])]).is_err());
        assert!(difference(vec![l(vec![]), Value::Null]).is_err());
        assert!(difference(vec![Value::Int(1), l(vec![])]).is_err());
        assert!(difference(vec![Value::Int(1), Value::Int(2)]).is_err());

        let r = disjunct(vec![
            l(vec![Value::Int(1), Value::Int(2)]),
            l(vec![Value::Int(2), Value::Int(3)]),
        ])
        .unwrap();
        if let Value::List(items) = r {
            assert!(items.contains(&Value::Int(1)));
            assert!(items.contains(&Value::Int(3)));
        }
        assert!(disjunct(vec![Value::Null, l(vec![])]).is_err());
        assert!(disjunct(vec![l(vec![]), Value::Null]).is_err());
        assert!(disjunct(vec![Value::Int(1), l(vec![])]).is_err());
        assert!(disjunct(vec![Value::Int(1), Value::Int(2)]).is_err());
    }

    #[test]
    fn product_basic_and_empty() {
        let r = product(vec![
            l(vec![Value::Int(1), Value::Int(2)]),
            l(vec![Value::Int(10), Value::Int(20)]),
        ])
        .unwrap();
        assert_eq!(
            r,
            l(vec![
                l(vec![Value::Int(1), Value::Int(10)]),
                l(vec![Value::Int(1), Value::Int(20)]),
                l(vec![Value::Int(2), Value::Int(10)]),
                l(vec![Value::Int(2), Value::Int(20)]),
            ])
        );
        assert_eq!(
            product(vec![l(vec![]), l(vec![Value::Int(1)])]).unwrap(),
            l(vec![])
        );
        assert!(product(vec![Value::Null, l(vec![])]).is_err());
        assert!(product(vec![l(vec![]), Value::Null]).is_err());
        assert!(product(vec![Value::Int(1), l(vec![])]).is_err());
        assert!(product(vec![Value::Int(1), Value::Int(2)]).is_err());
    }
}
