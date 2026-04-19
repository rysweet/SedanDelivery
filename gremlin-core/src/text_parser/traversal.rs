//! Inner-traversal argument parsing helpers (multi, option, from/to).

use super::*;
use super::{literal::*, tokenizer::*};

pub(crate) fn parse_multi_traversal_args(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Vec<Vec<GremlinStep>>, TextParseError> {
    let raw_args = split_top_level_args(args_str);
    let mut result = Vec::new();
    for arg in &raw_args {
        let inner = parse_inner_traversal(arg.trim(), resolve_param)?;
        result.push(inner);
    }
    Ok(result)
}

/// Parse `option(key, traversal)` — skip the first arg (key) and return the traversal.
pub(crate) fn parse_option_traversal(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Vec<GremlinStep>, TextParseError> {
    let raw_args = split_top_level_args(args_str);
    if raw_args.len() >= 2 {
        // The second argument onward is the traversal
        let traversal_str = raw_args[1..].join(",");
        parse_inner_traversal(traversal_str.trim(), resolve_param)
    } else {
        Ok(vec![GremlinStep::Identity])
    }
}

/// Parse a `from(...)` or `to(...)` argument — could be a string label or a traversal.
pub(crate) fn parse_from_to_traversal(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Vec<GremlinStep>, TextParseError> {
    let s = args_str.trim();
    // If it's a quoted string, it's a step label reference
    if s.starts_with('"') || s.starts_with('\'') {
        let label = unquote(s);
        return Ok(vec![GremlinStep::Select(None, vec![label])]);
    }
    // Handle to(Direction.OUT, "label") / to(Direction.IN, "label") / to(Direction.BOTH, "label")
    // These are equivalent to out("label") / in("label") / both("label")
    let args = split_top_level_args(s);
    if !args.is_empty() {
        let first = args[0].trim();
        if first.starts_with("Direction.") || first == "OUT" || first == "IN" || first == "BOTH" {
            let labels: Vec<String> = args[1..].iter().map(|a| unquote(a.trim())).collect();
            return match first {
                "Direction.OUT" | "OUT" => Ok(vec![GremlinStep::Out(labels)]),
                "Direction.IN" | "IN" => Ok(vec![GremlinStep::In(labels)]),
                "Direction.BOTH" | "BOTH" => Ok(vec![GremlinStep::Both(labels)]),
                _ => Ok(vec![GremlinStep::Both(labels)]),
            };
        }
    }
    // Otherwise parse as a full traversal
    let inner = parse_inner_traversal(s, resolve_param)?;
    if inner.is_empty() {
        Ok(vec![GremlinStep::Select(None, vec![])])
    } else {
        Ok(inner)
    }
}
