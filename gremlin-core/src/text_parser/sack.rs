//! `withSack(...)` initial value & operator parsing.

use super::literal::*;
use super::*;

pub(crate) fn extract_with_sack_value(
    input: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Option<(GremlinValue, Option<String>)> {
    let prefix = "g.withSack(";
    if !input.starts_with(prefix) {
        if let Some(pos) = input.find(".withSack(") {
            let after = &input[pos + ".withSack(".len()..];
            return parse_sack_arg(after, resolve_param);
        }
        return None;
    }
    let after = &input[prefix.len()..];
    parse_sack_arg(after, resolve_param)
}

pub(crate) fn parse_sack_arg(
    after: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Option<(GremlinValue, Option<String>)> {
    let mut depth = 1;
    let mut end = 0;
    for (i, c) in after.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if end == 0 {
        return None;
    }
    let arg_str = after[..end].trim();
    let first_arg = arg_str.split(',').next().unwrap_or(arg_str).trim();
    let merge_op = {
        let rest = &arg_str[first_arg.len()..];
        if let Some(after_comma) = rest.strip_prefix(',') {
            extract_operator_name(after_comma.trim())
        } else {
            None
        }
    };
    parse_literal_value(first_arg, resolve_param).map(|v| (v, merge_op))
}

/// Extract operator name from strings like "Operator.sum", "Operator.mult", etc.
pub(crate) fn extract_operator_name(s: &str) -> Option<String> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("Operator.") {
        let name = rest
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .next()?;
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    let known = ["sum", "minus", "mult", "div", "min", "max", "assign"];
    for op in &known {
        if s.starts_with(op) {
            return Some((*op).to_string());
        }
    }
    None
}
