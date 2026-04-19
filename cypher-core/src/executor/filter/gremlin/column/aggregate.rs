//! Local aggregations: sum, min, max, mean over a list (or scalar pass-through).

#![allow(clippy::needless_pass_by_value)]

use crate::error::GqlError;
use crate::types::Value;

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
                        if let Ok(std::cmp::Ordering::Less) =
                            super::super::super::cypher_compare(v, &cur)
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
                            super::super::super::cypher_compare(v, &cur)
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
