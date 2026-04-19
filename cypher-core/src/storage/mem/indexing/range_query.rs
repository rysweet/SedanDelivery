//! BTreeMap range query helpers for the in-memory property index.

use crate::storage::{NodeId, RangeQuery};

use super::super::{IndexableValue, PropIndex};

/// Collect node IDs from the BTreeMap property index that match the given range query.
pub(crate) fn indexed_range_query(
    prop_index: &PropIndex,
    label: &str,
    property: &str,
    range: &RangeQuery<'_>,
) -> Vec<NodeId> {
    use std::ops::Bound;

    let l = label.to_string();
    let p = property.to_string();

    // Helper: collect all node IDs from a BTreeMap range.
    macro_rules! collect_range {
        ($lo:expr, $hi:expr) => {{
            let mut result = Vec::new();
            for ((_l, _p, _v), ids) in prop_index.range(($lo, $hi)) {
                if _l != &l || _p != &p {
                    continue;
                }
                for &raw_id in ids {
                    result.push(NodeId(raw_id));
                }
            }
            result
        }};
    }

    // The smallest possible key for this (label, property).
    let min_key = (l.clone(), p.clone(), IndexableValue::Bool(false));

    match range {
        RangeQuery::Eq(target) => {
            if let Some(iv) = IndexableValue::from_value(target) {
                let key = (l, p, iv);
                prop_index
                    .get(&key)
                    .map(|ids| ids.iter().copied().map(NodeId).collect())
                    .unwrap_or_default()
            } else {
                vec![]
            }
        }
        RangeQuery::Lt(bound) => {
            if let Some(iv) = IndexableValue::from_value(bound) {
                let lo = Bound::Included(min_key);
                let hi = Bound::Excluded((l.clone(), p.clone(), iv));
                collect_range!(lo, hi)
            } else {
                vec![]
            }
        }
        RangeQuery::Lte(bound) => {
            if let Some(iv) = IndexableValue::from_value(bound) {
                let lo = Bound::Included(min_key);
                let hi = Bound::Included((l.clone(), p.clone(), iv));
                collect_range!(lo, hi)
            } else {
                vec![]
            }
        }
        RangeQuery::Gt(bound) => {
            if let Some(iv) = IndexableValue::from_value(bound) {
                let mut result = Vec::new();
                for ((kl, kp, _kv), ids) in prop_index.range((
                    Bound::Excluded((l.clone(), p.clone(), iv)),
                    Bound::Unbounded,
                )) {
                    if kl != &l || kp != &p {
                        break;
                    }
                    for &raw_id in ids {
                        result.push(NodeId(raw_id));
                    }
                }
                result
            } else {
                vec![]
            }
        }
        RangeQuery::Gte(bound) => {
            if let Some(iv) = IndexableValue::from_value(bound) {
                let mut result = Vec::new();
                for ((kl, kp, _kv), ids) in prop_index.range((
                    Bound::Included((l.clone(), p.clone(), iv)),
                    Bound::Unbounded,
                )) {
                    if kl != &l || kp != &p {
                        break;
                    }
                    for &raw_id in ids {
                        result.push(NodeId(raw_id));
                    }
                }
                result
            } else {
                vec![]
            }
        }
        RangeQuery::Between { lo, hi, inclusive } => {
            let lo_iv = IndexableValue::from_value(lo);
            let hi_iv = IndexableValue::from_value(hi);
            match (lo_iv, hi_iv) {
                (Some(lo_v), Some(hi_v)) => {
                    let lo_bound = Bound::Included((l.clone(), p.clone(), lo_v));
                    let hi_bound = if *inclusive {
                        Bound::Included((l.clone(), p.clone(), hi_v))
                    } else {
                        Bound::Excluded((l.clone(), p.clone(), hi_v))
                    };
                    collect_range!(lo_bound, hi_bound)
                }
                _ => vec![],
            }
        }
        RangeQuery::StartsWith(prefix) => {
            // Scan string range: from prefix to prefix with last byte incremented.
            let lo_v = IndexableValue::Str((*prefix).into());
            let mut result = Vec::new();
            for ((kl, kp, kv), ids) in prop_index.range((
                Bound::Included((l.clone(), p.clone(), lo_v)),
                Bound::Unbounded,
            )) {
                if kl != &l || kp != &p {
                    break;
                }
                if let IndexableValue::Str(s) = kv {
                    if s.starts_with(prefix) {
                        for &raw_id in ids {
                            result.push(NodeId(raw_id));
                        }
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            result
        }
    }
}
