//! Gremlin `math()` step expression evaluator.
//!
//! Behavior preserved verbatim from the original `filter` module.

use crate::error::GqlError;
use crate::types::Value;

/// Evaluate a Gremlin math expression. `_` refers to the current value.
/// `var_bindings` provides named variable values (e.g., from by() modulation or side effects).
pub(crate) fn eval_math_expr(
    expr: &str,
    current: &Value,
    var_bindings: &[(String, f64)],
) -> Result<Value, GqlError> {
    let expr = expr.trim();

    // Helper to extract numeric value
    fn as_f64(v: &Value) -> Option<f64> {
        match v {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    // Helper to resolve a token to a numeric value
    let resolve = |token: &str| -> f64 {
        if token == "_" {
            return as_f64(current).unwrap_or(0.0);
        }
        if let Ok(n) = token.parse::<f64>() {
            return n;
        }
        // Look up in variable bindings
        for (name, val) in var_bindings {
            if name == token {
                return *val;
            }
        }
        // Default to current value
        as_f64(current).unwrap_or(0.0)
    };

    let val = as_f64(current).unwrap_or(0.0);

    // ceil(_), floor(_), abs(_)
    if expr.starts_with("ceil(") || expr.starts_with("ceil ") || expr == "ceil" {
        return Ok(Value::Float(val.ceil()));
    }
    if expr.starts_with("floor(") || expr.starts_with("floor ") || expr == "floor" {
        return Ok(Value::Float(val.floor()));
    }
    if expr.starts_with("abs(") || expr.starts_with("abs ") || expr == "abs" {
        return Ok(Value::Float(val.abs()));
    }
    if expr.starts_with("sqrt(") || expr.starts_with("sqrt ") || expr == "sqrt" {
        return Ok(Value::Float(val.sqrt()));
    }

    // Binary operations: _ op N
    for (op_str, op_fn) in &[
        (" + ", (|a: f64, b: f64| a + b) as fn(f64, f64) -> f64),
        (" - ", |a, b| a - b),
        (" * ", |a, b| a * b),
        (" / ", |a, b| if b != 0.0 { a / b } else { f64::NAN }),
        (" % ", |a, b| if b != 0.0 { a % b } else { f64::NAN }),
    ] {
        if let Some(pos) = expr.find(op_str) {
            let left_str = expr[..pos].trim();
            let right_str = expr[pos + op_str.len()..].trim();

            let left_val = resolve(left_str);
            let right_val = resolve(right_str);

            let result = op_fn(left_val, right_val);
            return Ok(Value::Float(result));
        }
    }

    // Fallback: return current value unchanged
    Ok(current.clone())
}
