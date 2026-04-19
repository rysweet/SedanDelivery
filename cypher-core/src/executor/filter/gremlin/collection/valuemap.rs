//! TinkerPop `valueMap` projection helpers and related list/value extractors.

#![allow(clippy::needless_pass_by_value)]

use indexmap::IndexMap;

use crate::error::GqlError;
use crate::types::Value;

pub(crate) fn wraplist(vals: Vec<Value>) -> Result<Value, GqlError> {
    // Wrap a single value in a list for valueMap
    match vals.into_iter().next() {
        Some(v) => Ok(Value::List(vec![v])),
        None => Ok(Value::List(Vec::new())),
    }
}

pub(crate) fn valuemap(vals: Vec<Value>) -> Result<Value, GqlError> {
    // Full valueMap: extract all properties and wrap in lists.
    // Multi-valued properties (stored as List of Maps with "value" key)
    // are flattened to a list of the "value" fields, matching TinkerPop
    // multi-property semantics.
    match vals.into_iter().next() {
        Some(Value::Map(m)) => {
            let wrapped: IndexMap<Box<str>, Value> = m
                .into_iter()
                .map(|(k, v)| {
                    // Detect multi-property: List of Maps with "value" key
                    if let Value::List(ref items) = v {
                        let all_maps_with_value = !items.is_empty()
                            && items.iter().all(
                                |item| matches!(item, Value::Map(m) if m.contains_key("value")),
                            );
                        if all_maps_with_value {
                            let extracted: Vec<Value> = items
                                .iter()
                                .filter_map(|item| match item {
                                    Value::Map(m) => m.get("value").cloned(),
                                    _ => None,
                                })
                                .collect();
                            return (k, Value::List(extracted));
                        }
                    }
                    (k, Value::List(vec![v]))
                })
                .collect();
            Ok(Value::Map(wrapped))
        }
        Some(other) => Ok(other),
        None => Ok(Value::Null),
    }
}

pub(crate) fn valuemap_unfold(vals: Vec<Value>) -> Result<Value, GqlError> {
    // Unwrap list values in a valueMap result (by(__.unfold()))
    match vals.into_iter().next() {
        Some(Value::Map(m)) => {
            let unwrapped: IndexMap<Box<str>, Value> = m
                .into_iter()
                .map(|(k, v)| {
                    let unwrapped = match v {
                        Value::List(items) if items.len() == 1 => {
                            items.into_iter().next().unwrap_or(Value::Null)
                        }
                        other => other,
                    };
                    (k, unwrapped)
                })
                .collect();
            Ok(Value::Map(unwrapped))
        }
        Some(other) => Ok(other),
        None => Ok(Value::Null),
    }
}

pub(crate) fn values_all(vals: Vec<Value>) -> Result<Value, GqlError> {
    // Fallback for non-graph values (maps, etc.)
    match vals.into_iter().next() {
        Some(Value::Map(m)) => {
            // Property Map from properties(): extract ~value
            if m.contains_key("~value") {
                let val = m.get("~value").cloned().unwrap_or(Value::Null);
                Ok(Value::List(vec![val]))
            } else {
                let vals: Vec<Value> = m.into_values().collect();
                Ok(Value::List(vals))
            }
        }
        Some(Value::Null) | None => Ok(Value::List(vec![])),
        Some(other) => Ok(Value::List(vec![other])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(items: Vec<Value>) -> Value {
        Value::List(items)
    }

    #[test]
    fn wraplist_and_value_helpers() {
        assert_eq!(
            wraplist(vec![Value::Int(7)]).unwrap(),
            l(vec![Value::Int(7)])
        );
        assert_eq!(wraplist(vec![]).unwrap(), l(vec![]));
    }

    #[test]
    fn valuemap_wraps_singletons_and_flattens_multivalue() {
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("name".into(), Value::Str("alice".into()));
        // multi-property: list of maps with "value"
        let mut p1 = IndexMap::<Box<str>, Value>::new();
        p1.insert("value".into(), Value::Int(1));
        let mut p2 = IndexMap::<Box<str>, Value>::new();
        p2.insert("value".into(), Value::Int(2));
        m.insert("scores".into(), l(vec![Value::Map(p1), Value::Map(p2)]));
        let out = valuemap(vec![Value::Map(m)]).unwrap();
        if let Value::Map(out) = out {
            assert_eq!(out.get("name"), Some(&l(vec![Value::Str("alice".into())])));
            assert_eq!(
                out.get("scores"),
                Some(&l(vec![Value::Int(1), Value::Int(2)]))
            );
        } else {
            panic!("expected map");
        }
        // non-map passes through
        assert_eq!(valuemap(vec![Value::Int(7)]).unwrap(), Value::Int(7));
        assert_eq!(valuemap(vec![]).unwrap(), Value::Null);
    }

    #[test]
    fn valuemap_unfold_unwraps_singletons() {
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("a".into(), l(vec![Value::Int(7)]));
        m.insert("b".into(), l(vec![Value::Int(1), Value::Int(2)]));
        let out = valuemap_unfold(vec![Value::Map(m)]).unwrap();
        if let Value::Map(out) = out {
            assert_eq!(out.get("a"), Some(&Value::Int(7)));
            assert_eq!(out.get("b"), Some(&l(vec![Value::Int(1), Value::Int(2)])));
        }
        assert_eq!(valuemap_unfold(vec![Value::Int(7)]).unwrap(), Value::Int(7));
        assert_eq!(valuemap_unfold(vec![]).unwrap(), Value::Null);
    }

    #[test]
    fn values_all_variants() {
        // Map with ~value tag
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("~value".into(), Value::Int(7));
        assert_eq!(
            values_all(vec![Value::Map(m)]).unwrap(),
            l(vec![Value::Int(7)])
        );
        // Map without ~value -> all values
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("a".into(), Value::Int(1));
        m.insert("b".into(), Value::Int(2));
        assert_eq!(
            values_all(vec![Value::Map(m)]).unwrap(),
            l(vec![Value::Int(1), Value::Int(2)])
        );
        // Null/None
        assert_eq!(values_all(vec![Value::Null]).unwrap(), l(vec![]));
        assert_eq!(values_all(vec![]).unwrap(), l(vec![]));
        // Other scalar
        assert_eq!(
            values_all(vec![Value::Int(7)]).unwrap(),
            l(vec![Value::Int(7)])
        );
    }
}
