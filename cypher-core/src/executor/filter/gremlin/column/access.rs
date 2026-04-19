//! Column accessor functions: key/value/keys/values extraction and select(values).

#![allow(clippy::needless_pass_by_value)]

use super::super::super::map_key::map_key_to_value;
use crate::error::GqlError;
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
