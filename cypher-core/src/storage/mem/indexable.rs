//! Ord-compatible wrapper for property index keys, plus the `BTreeMap`-backed
//! property index type aliases used across the in-memory storage backend.

use std::collections::{BTreeMap, HashSet};

use crate::types::Value;

/// A value that can be used as a BTreeMap key for property indexes.
/// Wraps the subset of `Value` variants that support total ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IndexableValue {
    Bool(bool),
    Int(i64),
    /// f64 stored as raw bits for Ord. NaN values are excluded at insertion time.
    Float(u64),
    Str(Box<str>),
}

impl IndexableValue {
    /// Convert a `Value` to an `IndexableValue`, returning `None` for non-indexable types.
    pub(crate) fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Bool(b) => Some(IndexableValue::Bool(*b)),
            Value::Int(i) => Some(IndexableValue::Int(*i)),
            Value::Float(f) => {
                if f.is_nan() {
                    None
                } else {
                    Some(IndexableValue::Float(f.to_bits()))
                }
            }
            Value::Str(s) => Some(IndexableValue::Str(s.clone())),
            _ => None,
        }
    }
}

impl PartialOrd for IndexableValue {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for IndexableValue {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let disc = |v: &IndexableValue| -> u8 {
            match v {
                IndexableValue::Bool(_) => 0,
                IndexableValue::Int(_) => 1,
                IndexableValue::Float(_) => 2,
                IndexableValue::Str(_) => 3,
            }
        };
        let d1 = disc(self);
        let d2 = disc(other);
        if d1 != d2 {
            return d1.cmp(&d2);
        }
        match (self, other) {
            (IndexableValue::Bool(a), IndexableValue::Bool(b)) => a.cmp(b),
            (IndexableValue::Int(a), IndexableValue::Int(b)) => a.cmp(b),
            (IndexableValue::Float(a), IndexableValue::Float(b)) => {
                let fa = f64::from_bits(*a);
                let fb = f64::from_bits(*b);
                fa.total_cmp(&fb)
            }
            (IndexableValue::Str(a), IndexableValue::Str(b)) => a.cmp(b),
            _ => unreachable!(),
        }
    }
}

/// Composite key for the property index BTreeMap: (label, property, value).
pub(crate) type PropIndexKey = (String, String, IndexableValue);

/// The property index: maps (label, property, value) -> set of raw node IDs.
pub(crate) type PropIndex = BTreeMap<PropIndexKey, HashSet<u64>>;
