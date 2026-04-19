//! Collection-shaped argument parsing: arg lists, string lists, and collection-step args.

use super::super::tokenizer::*;
use super::super::*;
use super::string::unquote;
use super::value::parse_single_value;

/// Parse a collection step argument. Handles both literal values (null, 2, [1,2,3])
/// and sub-traversals (__.V().values("age").fold()).
pub(crate) fn parse_collection_step_arg(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Vec<GremlinStep>, TextParseError> {
    let s = args_str.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    // Detect literal null
    if s == "null" {
        return Ok(vec![GremlinStep::Constant(GremlinValue::Null)]);
    }
    // Detect list or map literal [...]
    if s.starts_with('[') && s.ends_with(']') {
        // Use parse_single_value which correctly distinguishes [k:v] maps
        // from [a,b,c] lists by checking for top-level colons.
        if let Ok(val) = parse_single_value(s, resolve_param) {
            return Ok(vec![GremlinStep::Inject(vec![val])]);
        }
    }
    // Detect simple numeric/boolean literals (not a traversal)
    if let Ok(n) = s.parse::<i64>() {
        return Ok(vec![GremlinStep::Constant(GremlinValue::Integer(n))]);
    }
    if let Ok(f) = s.parse::<f64>() {
        return Ok(vec![GremlinStep::Constant(GremlinValue::Float(f))]);
    }
    if s == "true" {
        return Ok(vec![GremlinStep::Constant(GremlinValue::Bool(true))]);
    }
    if s == "false" {
        return Ok(vec![GremlinStep::Constant(GremlinValue::Bool(false))]);
    }
    // Detect quoted string literal
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        let unescaped = inner.replace("\\\"", "\"").replace("\\\\", "\\");
        return Ok(vec![GremlinStep::Constant(GremlinValue::String(unescaped))]);
    }
    // Numeric with type suffix
    if s.len() > 1 {
        let suffix = s.as_bytes()[s.len() - 1];
        let num_part = &s[..s.len() - 1];
        if matches!(suffix, b'b' | b's' | b'i' | b'l' | b'L' | b'n') {
            if let Ok(n) = num_part.parse::<i64>() {
                return Ok(vec![GremlinStep::Constant(GremlinValue::Integer(n))]);
            }
        }
        if matches!(suffix, b'f' | b'd') {
            if let Ok(f) = num_part.parse::<f64>() {
                return Ok(vec![GremlinStep::Constant(GremlinValue::Float(f))]);
            }
        }
    }
    // Fall back to sub-traversal parsing
    parse_inner_traversal(args_str, resolve_param)
}

/// Parse multiple traversal arguments separated by commas at the top level.
pub(crate) fn parse_arg_list(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Vec<GremlinValue>, TextParseError> {
    if args_str.trim().is_empty() {
        return Ok(Vec::new());
    }

    let raw_args = split_top_level_args(args_str);
    let mut values = Vec::new();
    for arg in raw_args {
        values.push(parse_single_value(arg.trim(), resolve_param)?);
    }
    Ok(values)
}

/// Parse a list of string-only arguments.
pub(crate) fn parse_string_arg_list(args_str: &str) -> Result<Vec<String>, TextParseError> {
    if args_str.trim().is_empty() {
        return Ok(Vec::new());
    }

    let raw_args = split_top_level_args(args_str);
    let mut result = Vec::new();
    for arg in raw_args {
        let s = arg.trim();
        result.push(unquote(s));
    }
    Ok(result)
}

/// Parse a list of string arguments, resolving parameter references.
pub(crate) fn parse_string_arg_list_with_params(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Vec<String>, TextParseError> {
    if args_str.trim().is_empty() {
        return Ok(Vec::new());
    }
    let raw_args = split_top_level_args(args_str);
    let mut result = Vec::new();
    for arg in raw_args {
        let s = arg.trim();
        if let Some(GremlinValue::String(val)) = resolve_param(s) {
            result.push(val);
        } else {
            result.push(unquote(s));
        }
    }
    Ok(result)
}
