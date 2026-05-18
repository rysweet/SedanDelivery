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
        // Use s.get() instead of &s[..] to avoid panic when the slice
        // boundary falls inside a multi-byte UTF-8 character (fuzz fix).
        let num_part = match s.get(..s.len() - 1) {
            Some(p) => p,
            None => return parse_inner_traversal(args_str, resolve_param),
        };
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

#[cfg(test)]
mod collection_utf8_safety_tests {
    use super::parse_collection_step_arg;

    fn no_params(_: &str) -> Option<crate::types::GremlinValue> {
        None
    }

    #[test]
    fn numeric_suffix_with_multibyte_last_char_does_not_panic() {
        let _ = parse_collection_step_arg("42é", &no_params);
    }

    #[test]
    fn numeric_suffix_with_3byte_char_does_not_panic() {
        let _ = parse_collection_step_arg("7€", &no_params);
    }

    #[test]
    fn numeric_suffix_with_4byte_char_does_not_panic() {
        let _ = parse_collection_step_arg("9𝕏", &no_params);
    }

    #[test]
    fn single_multibyte_char_does_not_panic() {
        let _ = parse_collection_step_arg("é", &no_params);
    }

    #[test]
    fn numeric_suffix_with_ascii_suffix_still_works() {
        let result = parse_collection_step_arg("42L", &no_params).unwrap();
        assert_eq!(result.len(), 1);
        match &result[0] {
            crate::bytecode::GremlinStep::Constant(crate::types::GremlinValue::Integer(n)) => {
                assert_eq!(*n, 42)
            }
            other => panic!("expected Constant(Integer(42)), got {:?}", other),
        }
    }

    #[test]
    fn numeric_suffix_with_float_suffix_still_works() {
        let result = parse_collection_step_arg("2.75f", &no_params).unwrap();
        assert_eq!(result.len(), 1);
        match &result[0] {
            crate::bytecode::GremlinStep::Constant(crate::types::GremlinValue::Float(f)) => {
                assert!((f - 2.75).abs() < f64::EPSILON)
            }
            other => panic!("expected Constant(Float(2.75)), got {:?}", other),
        }
    }

    #[test]
    fn empty_string_does_not_panic() {
        let result = parse_collection_step_arg("", &no_params).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn single_ascii_char_does_not_panic() {
        let _ = parse_collection_step_arg("L", &no_params);
    }

    #[test]
    fn fuzz_regression_cyrillic_suffix() {
        let _ = parse_collection_step_arg("0ӊ", &no_params);
    }
}
