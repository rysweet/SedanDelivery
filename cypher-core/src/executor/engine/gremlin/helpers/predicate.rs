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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Value;

    // ── eval_select_math_expr ────────────────────────────────────────

    #[test]
    fn math_self_add_int() {
        assert_eq!(
            eval_select_math_expr("_+_", &Value::Int(5)),
            Value::Float(10.0)
        );
    }

    #[test]
    fn math_self_add_float() {
        assert_eq!(
            eval_select_math_expr("_+_", &Value::Float(2.5)),
            Value::Float(5.0)
        );
    }

    #[test]
    fn math_self_sub_is_zero() {
        assert_eq!(
            eval_select_math_expr("_-_", &Value::Int(7)),
            Value::Float(0.0)
        );
    }

    #[test]
    fn math_self_mul() {
        assert_eq!(
            eval_select_math_expr("_*_", &Value::Int(4)),
            Value::Float(16.0)
        );
    }

    #[test]
    fn math_self_div_nonzero() {
        assert_eq!(
            eval_select_math_expr("_/_", &Value::Int(3)),
            Value::Float(1.0)
        );
    }

    #[test]
    fn math_self_div_zero_is_nan() {
        if let Value::Float(f) = eval_select_math_expr("_/_", &Value::Int(0)) {
            assert!(f.is_nan());
        } else {
            panic!("expected NaN float");
        }
    }

    #[test]
    fn math_self_mod_nonzero_is_zero() {
        assert_eq!(
            eval_select_math_expr("_%_", &Value::Int(3)),
            Value::Float(0.0)
        );
    }

    #[test]
    fn math_self_mod_zero_is_nan() {
        if let Value::Float(f) = eval_select_math_expr("_%_", &Value::Int(0)) {
            assert!(f.is_nan());
        } else {
            panic!("expected NaN float");
        }
    }

    #[test]
    fn math_with_whitespace_normalised() {
        assert_eq!(
            eval_select_math_expr("  _ + _  ", &Value::Int(2)),
            Value::Float(4.0)
        );
    }

    #[test]
    fn math_unknown_expr_returns_self() {
        assert_eq!(eval_select_math_expr("_!_", &Value::Int(5)), Value::Int(5));
    }

    #[test]
    fn math_non_numeric_returns_self() {
        assert_eq!(
            eval_select_math_expr("_+_", &Value::Str("x".into())),
            Value::Str("x".into())
        );
    }

    // ── eval_gremlin_predicate ───────────────────────────────────────

    #[test]
    fn pred_between_inclusive_lower() {
        assert!(eval_gremlin_predicate("P.between(10,20)", &Value::Int(10)));
    }

    #[test]
    fn pred_between_exclusive_upper() {
        assert!(!eval_gremlin_predicate("P.between(10,20)", &Value::Int(20)));
    }

    #[test]
    fn pred_between_outside() {
        assert!(!eval_gremlin_predicate("P.between(10,20)", &Value::Int(5)));
    }

    #[test]
    fn pred_between_malformed() {
        assert!(!eval_gremlin_predicate("P.between(10)", &Value::Int(15)));
        assert!(!eval_gremlin_predicate("P.between(a,b)", &Value::Int(15)));
    }

    #[test]
    fn pred_lt_gt_lte_gte() {
        assert!(eval_gremlin_predicate("P.lt(30)", &Value::Int(20)));
        assert!(!eval_gremlin_predicate("P.lt(30)", &Value::Int(30)));
        assert!(eval_gremlin_predicate("P.lte(30)", &Value::Int(30)));
        assert!(eval_gremlin_predicate("P.gt(30)", &Value::Int(40)));
        assert!(!eval_gremlin_predicate("P.gt(30)", &Value::Int(30)));
        assert!(eval_gremlin_predicate("P.gte(30)", &Value::Int(30)));
    }

    #[test]
    fn pred_eq_neq_numeric() {
        assert!(eval_gremlin_predicate("P.eq(5)", &Value::Int(5)));
        assert!(eval_gremlin_predicate("P.eq(5)", &Value::Float(5.0)));
        assert!(eval_gremlin_predicate("P.neq(5)", &Value::Int(6)));
    }

    #[test]
    fn pred_eq_string_match() {
        assert!(eval_gremlin_predicate(
            r#"P.eq("hello")"#,
            &Value::Str("hello".into())
        ));
        assert!(!eval_gremlin_predicate(
            r#"P.eq("hello")"#,
            &Value::Str("world".into())
        ));
        // eq on non-string non-numeric value returns false
        assert!(!eval_gremlin_predicate(r#"P.eq("x")"#, &Value::Bool(true)));
    }

    #[test]
    fn pred_neq_string_returns_true_for_other_types() {
        assert!(eval_gremlin_predicate(
            r#"P.neq("hello")"#,
            &Value::Str("world".into())
        ));
        assert!(eval_gremlin_predicate(r#"P.neq("x")"#, &Value::Bool(true)));
    }

    #[test]
    fn pred_unknown_returns_false() {
        assert!(!eval_gremlin_predicate("P.unknown(1)", &Value::Int(1)));
    }

    #[test]
    fn pred_compound_or() {
        assert!(eval_gremlin_predicate(
            "P.lt(10).or(P.gt(20))",
            &Value::Int(5)
        ));
        assert!(eval_gremlin_predicate(
            "P.lt(10).or(P.gt(20))",
            &Value::Int(25)
        ));
        assert!(!eval_gremlin_predicate(
            "P.lt(10).or(P.gt(20))",
            &Value::Int(15)
        ));
    }

    #[test]
    fn pred_compound_and() {
        assert!(eval_gremlin_predicate(
            "P.gt(10).and(P.lt(20))",
            &Value::Int(15)
        ));
        assert!(!eval_gremlin_predicate(
            "P.gt(10).and(P.lt(20))",
            &Value::Int(5)
        ));
    }

    #[test]
    fn pred_compound_without_p_prefix_on_inner() {
        assert!(eval_gremlin_predicate("gt(10).or(lt(2))", &Value::Int(15)));
    }

    #[test]
    fn pred_no_p_prefix() {
        assert!(eval_gremlin_predicate("between(0,5)", &Value::Int(3)));
    }

    // ── parse_gremlin_option_args / value / trim ─────────────────────

    #[test]
    fn parse_args_simple() {
        let v = parse_gremlin_option_args("a, b, c");
        assert_eq!(v, vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_args_respects_double_quotes() {
        let v = parse_gremlin_option_args(r#""a,b", c"#);
        assert_eq!(v, vec![r#""a,b""#, "c"]);
    }

    #[test]
    fn parse_args_respects_single_quotes() {
        let v = parse_gremlin_option_args("'a,b', c");
        assert_eq!(v, vec!["'a,b'", "c"]);
    }

    #[test]
    fn parse_args_empty_input() {
        let v = parse_gremlin_option_args("");
        assert!(v.is_empty());
    }

    #[test]
    fn parse_args_trailing_whitespace_trimmed() {
        let v = parse_gremlin_option_args("  one   ");
        assert_eq!(v, vec!["one"]);
    }

    #[test]
    fn trim_strips_quotes() {
        assert_eq!(trim_gremlin_option_string(r#"  "abc"  "#), "abc");
        assert_eq!(trim_gremlin_option_string("  'x'  "), "x");
    }

    #[test]
    fn parse_value_string_literal_double() {
        assert_eq!(
            parse_gremlin_option_value(r#""hi""#),
            Value::Str("hi".into())
        );
    }

    #[test]
    fn parse_value_string_literal_single() {
        assert_eq!(parse_gremlin_option_value("'hi'"), Value::Str("hi".into()));
    }

    #[test]
    fn parse_value_int_float_bool_null() {
        assert_eq!(parse_gremlin_option_value("42"), Value::Int(42));
        assert_eq!(parse_gremlin_option_value("3.5"), Value::Float(3.5));
        assert_eq!(parse_gremlin_option_value("true"), Value::Bool(true));
        assert_eq!(parse_gremlin_option_value("false"), Value::Bool(false));
        assert_eq!(parse_gremlin_option_value("null"), Value::Null);
    }

    #[test]
    fn parse_value_bare_word_falls_back_to_string() {
        assert_eq!(parse_gremlin_option_value("foo"), Value::Str("foo".into()));
    }

    // ── split_compound_predicate ─────────────────────────────────────

    #[test]
    fn split_returns_none_for_simple() {
        assert!(split_compound_predicate("between(1,2)").is_none());
        assert!(split_compound_predicate("lt(5)").is_none());
    }

    #[test]
    fn split_returns_or_pair() {
        let r = split_compound_predicate("lt(10).or(gt(20))").unwrap();
        assert_eq!(r, ("lt(10)", "gt(20)", true));
    }

    #[test]
    fn split_returns_and_pair() {
        let r = split_compound_predicate("gt(10).and(lt(20))").unwrap();
        assert_eq!(r, ("gt(10)", "lt(20)", false));
    }

    #[test]
    fn split_ignores_nested_parens() {
        // Outer .or( ) — the inner between() parens shouldn't trigger split
        let r = split_compound_predicate("between(0,5).or(gt(10))").unwrap();
        assert_eq!(r, ("between(0,5)", "gt(10)", true));
    }
}
