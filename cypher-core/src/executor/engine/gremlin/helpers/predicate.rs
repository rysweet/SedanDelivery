//! Parsing and evaluation of Gremlin `P.*` predicate strings, plus the
//! small helpers used for `option(...)` argument parsing and the
//! arithmetic-on-current-element shorthand used by `select(...).by(...)`.

use crate::types::Value;

pub(in crate::executor::engine::gremlin) fn eval_select_math_expr(
    expr: &str,
    current: &Value,
) -> Value {
    fn as_f64(v: &Value) -> Option<f64> {
        match v {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    let Some(current_num) = as_f64(current) else {
        return current.clone();
    };

    let compact: String = expr.chars().filter(|c| !c.is_whitespace()).collect();
    match compact.as_str() {
        "_+_" => Value::Float(current_num + current_num),
        "_-_" => Value::Float(0.0),
        "_*_" => Value::Float(current_num * current_num),
        "_/_" => Value::Float(if current_num != 0.0 { 1.0 } else { f64::NAN }),
        "_%_" => Value::Float(if current_num != 0.0 {
            current_num % current_num
        } else {
            f64::NAN
        }),
        _ => current.clone(),
    }
}

/// Evaluate a Gremlin predicate string (e.g., "P.between(26,30)") against a value.
pub(in crate::executor::engine::gremlin) fn eval_gremlin_predicate(
    pred_str: &str,
    value: &Value,
) -> bool {
    fn parse_num(s: &str) -> Option<f64> {
        let s = s.trim();
        if let Ok(i) = s.parse::<i64>() {
            return Some(i as f64);
        }
        s.parse::<f64>().ok()
    }
    fn value_as_f64(v: &Value) -> Option<f64> {
        match v {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    let pred = pred_str.strip_prefix("P.").unwrap_or(pred_str);

    // Handle compound predicates: .or(P.xxx) and .and(P.xxx)
    // Split at top-level .or() / .and() boundaries.
    if let Some((left, right, is_or)) = split_compound_predicate(pred) {
        let left_str = if left.starts_with("P.") {
            left.to_string()
        } else {
            format!("P.{left}")
        };
        let right_str = if right.starts_with("P.") {
            right.to_string()
        } else {
            format!("P.{right}")
        };
        let left_result = eval_gremlin_predicate(&left_str, value);
        let right_result = eval_gremlin_predicate(&right_str, value);
        return if is_or {
            left_result || right_result
        } else {
            left_result && right_result
        };
    }

    if let Some(rest) = pred.strip_prefix("between(") {
        if let Some(args) = rest.strip_suffix(')') {
            let parts: Vec<&str> = args.splitn(2, ',').collect();
            if parts.len() == 2 {
                if let (Some(lo), Some(hi), Some(v)) = (
                    parse_num(parts[0]),
                    parse_num(parts[1]),
                    value_as_f64(value),
                ) {
                    return v >= lo && v < hi;
                }
            }
        }
        return false;
    }

    // Single-arg predicates: P.lt(30), P.gt(30), P.lte(30), P.gte(30), P.eq(x), P.neq(x)
    for (prefix, cmp_fn) in &[
        ("lt(", (|v: f64, t: f64| v < t) as fn(f64, f64) -> bool),
        ("lte(", |v, t| v <= t),
        ("gt(", |v, t| v > t),
        ("gte(", |v, t| v >= t),
        ("eq(", |v, t| (v - t).abs() < 1e-9),
        ("neq(", |v, t| (v - t).abs() >= 1e-9),
    ] {
        if let Some(rest) = pred.strip_prefix(prefix) {
            if let Some(arg_str) = rest.strip_suffix(')') {
                // Try numeric comparison first
                if let (Some(threshold), Some(v)) = (parse_num(arg_str), value_as_f64(value)) {
                    return cmp_fn(v, threshold);
                }
                // For eq/neq, also try string comparison
                if *prefix == "eq(" {
                    let arg = arg_str.trim().trim_matches('"');
                    return match value {
                        Value::Str(s) => s.as_ref() == arg,
                        _ => false,
                    };
                }
                if *prefix == "neq(" {
                    let arg = arg_str.trim().trim_matches('"');
                    return match value {
                        Value::Str(s) => s.as_ref() != arg,
                        _ => true,
                    };
                }
            }
            return false;
        }
    }

    // P.neq(y) with string comparison
    if let Some(rest) = pred.strip_prefix("neq(") {
        if let Some(arg_str) = rest.strip_suffix(')') {
            let arg = arg_str.trim().trim_matches('"');
            match value {
                Value::Str(s) => return s.as_ref() != arg,
                _ => return true,
            }
        }
    }

    false
}

pub(in crate::executor::engine::gremlin) fn parse_gremlin_option_args(args: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    for ch in args.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                current.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                current.push(ch);
            }
            ',' if !in_single && !in_double => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

pub(in crate::executor::engine::gremlin) fn trim_gremlin_option_string(s: &str) -> &str {
    s.trim().trim_matches(|c| c == '"' || c == '\'')
}

pub(in crate::executor::engine::gremlin) fn parse_gremlin_option_value(raw: &str) -> Value {
    let trimmed = trim_gremlin_option_string(raw);
    if raw.trim().starts_with('"') || raw.trim().starts_with('\'') {
        return Value::Str(trimmed.into());
    }
    if let Ok(i) = trimmed.parse::<i64>() {
        return Value::Int(i);
    }
    if let Ok(f) = trimmed.parse::<f64>() {
        return Value::Float(f);
    }
    match trimmed {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" => Value::Null,
        _ => Value::Str(trimmed.into()),
    }
}

/// Split a compound predicate like "between(26, 30).or(gt(34))" into
/// (left_pred, right_pred, is_or). Returns None if not compound.
pub(in crate::executor::engine::gremlin) fn split_compound_predicate(
    pred: &str,
) -> Option<(&str, &str, bool)> {
    // Look for .or( or .and( at the top level (outside balanced parens).
    let mut depth = 0u32;
    let bytes = pred.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b'.' if depth == 0 && i + 4 <= len => {
                let suffix = &pred[i..];
                if suffix.starts_with(".or(") {
                    let left = &pred[..i];
                    let inner = &pred[i + 4..];
                    // Find matching closing paren for .or(
                    if let Some(close) = inner.strip_suffix(')') {
                        return Some((left, close, true));
                    }
                }
                if suffix.starts_with(".and(") {
                    let left = &pred[..i];
                    let inner = &pred[i + 5..];
                    if let Some(close) = inner.strip_suffix(')') {
                        return Some((left, close, false));
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}
