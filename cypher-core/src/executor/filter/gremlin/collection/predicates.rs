//! Collection predicates: `all`, `any`, `none`.

#![allow(clippy::needless_pass_by_value)]

use super::super::super::gremlin_pred::gremlin_pred_check;
use crate::error::GqlError;
use crate::types::Value;

pub(crate) fn all(vals: Vec<Value>) -> Result<Value, GqlError> {
    // all(pred): filter step — returns the list if ALL elements match, else filters out
    let list_val = vals.first().cloned().unwrap_or(Value::Null);
    match &list_val {
        Value::List(items) => {
            // pred info in vals[1] (op string) and vals[2] (value)
            let pred_op = match vals.get(1) {
                Some(Value::Str(s)) => s.as_ref(),
                _ => "neq",
            };
            let pred_val = vals.get(2).cloned().unwrap_or(Value::Null);
            let all_match = items
                .iter()
                .all(|v| gremlin_pred_check(v, pred_op, &pred_val));
            Ok(Value::Bool(all_match))
        }
        _ => Ok(Value::Bool(false)),
    }
}

pub(crate) fn any(vals: Vec<Value>) -> Result<Value, GqlError> {
    let list_val = vals.first().cloned().unwrap_or(Value::Null);
    match &list_val {
        Value::List(items) => {
            let pred_op = match vals.get(1) {
                Some(Value::Str(s)) => s.as_ref(),
                _ => "neq",
            };
            let pred_val = vals.get(2).cloned().unwrap_or(Value::Null);
            let any_match = items
                .iter()
                .any(|v| gremlin_pred_check(v, pred_op, &pred_val));
            Ok(Value::Bool(any_match))
        }
        _ => Ok(Value::Bool(false)),
    }
}

pub(crate) fn none(vals: Vec<Value>) -> Result<Value, GqlError> {
    let list_val = vals.first().cloned().unwrap_or(Value::Null);
    let pred_op = match vals.get(1) {
        Some(Value::Str(s)) => s.as_ref(),
        _ => "neq",
    };
    let pred_val = vals.get(2).cloned().unwrap_or(Value::Null);
    match &list_val {
        Value::List(items) => {
            let none_match = !items
                .iter()
                .any(|v| gremlin_pred_check(v, pred_op, &pred_val));
            Ok(Value::Bool(none_match))
        }
        // Scalar: none(P) is a collection predicate — always false
        // for non-collection values.
        _ => Ok(Value::Bool(false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(items: Vec<Value>) -> Value {
        Value::List(items)
    }

    #[test]
    fn all_any_none_pred() {
        // pred default ("neq" with Null) returns true for non-null elements
        assert_eq!(
            all(vec![l(vec![Value::Int(1), Value::Int(2)])]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            any(vec![l(vec![Value::Int(1), Value::Int(2)])]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            none(vec![l(vec![Value::Int(1), Value::Int(2)])]).unwrap(),
            Value::Bool(false)
        );
        // empty list: all → true, any → false, none → true
        assert_eq!(all(vec![l(vec![])]).unwrap(), Value::Bool(true));
        assert_eq!(any(vec![l(vec![])]).unwrap(), Value::Bool(false));
        assert_eq!(none(vec![l(vec![])]).unwrap(), Value::Bool(true));
        // non-list scalar
        assert_eq!(all(vec![Value::Int(1)]).unwrap(), Value::Bool(false));
        assert_eq!(any(vec![Value::Int(1)]).unwrap(), Value::Bool(false));
        assert_eq!(none(vec![Value::Int(1)]).unwrap(), Value::Bool(false));
    }
}
