//! Branch / control-flow steps: repeat, until, emit, times, union, choose, optional,
//! coalesce, local, branch, sideEffect, map, flatMap, match, option.

use super::super::*;
use super::super::{literal::*, traversal::*};

pub(super) fn try_dispatch(
    name: &str,
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<bool, TextParseError> {
    match name {
        "repeat" => {
            let trimmed = args_str.trim();
            if trimmed.starts_with('"') || trimmed.starts_with('\'') {
                let quote = trimmed.as_bytes()[0] as char;
                if let Some(end_quote) = trimmed[1..].find(quote) {
                    let label = &trimmed[1..1 + end_quote];
                    let rest = trimmed[1 + end_quote + 1..].trim();
                    let rest = rest.strip_prefix(',').unwrap_or(rest).trim();
                    let inner = parse_inner_traversal(rest, resolve_param)?;
                    out.push(GremlinStep::Repeat(Some(label.into()), inner));
                } else {
                    let inner = parse_inner_traversal(args_str, resolve_param)?;
                    out.push(GremlinStep::Repeat(None, inner));
                }
            } else {
                let inner = parse_inner_traversal(args_str, resolve_param)?;
                out.push(GremlinStep::Repeat(None, inner));
            }
        }
        "times" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if let Some(GremlinValue::Integer(n)) = args.first() {
                out.push(GremlinStep::Times(*n));
            }
        }
        "until" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Until(inner));
        }
        "emit" => {
            let trimmed = args_str.trim();
            if trimmed.is_empty() {
                out.push(GremlinStep::Emit);
            } else {
                let inner = parse_inner_traversal(trimmed, resolve_param)?;
                if inner.is_empty() {
                    out.push(GremlinStep::Emit);
                } else {
                    out.push(GremlinStep::EmitPred(inner));
                }
            }
        }
        "union" => {
            let traversals = parse_multi_traversal_args(args_str, resolve_param)?;
            out.push(GremlinStep::Union(traversals));
        }
        "coalesce" => {
            let traversals = parse_multi_traversal_args(args_str, resolve_param)?;
            out.push(GremlinStep::Coalesce(traversals));
        }
        "choose" => {
            let traversals = parse_multi_traversal_args(args_str, resolve_param)?;
            match traversals.len() {
                3 => out.push(GremlinStep::Choose(
                    traversals[0].clone(),
                    traversals[1].clone(),
                    Some(traversals[2].clone()),
                )),
                2 => out.push(GremlinStep::Choose(
                    traversals[0].clone(),
                    traversals[1].clone(),
                    None,
                )),
                1 => {
                    out.push(GremlinStep::Choose(traversals[0].clone(), Vec::new(), None));
                }
                _ => {
                    out.push(GremlinStep::Choose(Vec::new(), Vec::new(), None));
                }
            }
        }
        "optional" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Optional(inner));
        }
        "local" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Map(inner));
        }
        "branch" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Choose(inner, Vec::new(), None));
        }
        "sideEffect" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::SideEffect(inner));
        }
        "map" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Map(inner));
        }
        "flatMap" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::FlatMap(inner));
        }
        "match" => {
            let traversals = parse_multi_traversal_args(args_str, resolve_param)?;
            out.push(GremlinStep::Match(traversals));
        }
        "option" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if args.len() >= 2 {
                let token = args[0].clone();
                if let GremlinValue::String(ref s) = token {
                    let lower = s.to_lowercase();
                    if !s.starts_with("__")
                        && (lower.starts_with("is(")
                            || lower.starts_with("haslabel(")
                            || lower.starts_with("has(")
                            || lower.starts_with("out(")
                            || lower.starts_with("in(")
                            || lower.starts_with("both(")
                            || lower.starts_with("values(")
                            || lower.starts_with("identity("))
                    {
                        return Err(TextParseError::Expected {
                            expected: "Traversal is not allowed as a Pick token".into(),
                            found: s.clone(),
                        });
                    }
                }
                let inner_traversal = parse_option_traversal(args_str, resolve_param)?;
                out.push(GremlinStep::Option(token, inner_traversal));
            } else if args.len() == 1 {
                let token = args[0].clone();
                out.push(GremlinStep::Option(token, vec![GremlinStep::Identity]));
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}
