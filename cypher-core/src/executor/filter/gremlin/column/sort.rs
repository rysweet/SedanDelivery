//! Local sort for lists and maps with optional Column.keys/values selector.

#![allow(clippy::needless_pass_by_value)]

use super::super::super::gremlin_pred::extract_sort_key;
use crate::error::GqlError;
use crate::types::Value;

pub(crate) fn sort_local(vals: Vec<Value>) -> Result<Value, GqlError> {
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
