//! Vertex/edge traversal steps: out, in, both, outE, inE, bothE, inV, outV, bothV, otherV, toE.

use super::super::*;
use super::super::{literal::*, tokenizer::*};

pub(super) fn try_dispatch(
    name: &str,
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<bool, TextParseError> {
    match name {
        "out" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::Out(args));
        }
        "in" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::In(args));
        }
        "both" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::Both(args));
        }
        "outE" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::OutE(args));
        }
        "inE" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::InE(args));
        }
        "bothE" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::BothE(args));
        }
        "inV" => out.push(GremlinStep::InV),
        "outV" => out.push(GremlinStep::OutV),
        "bothV" => out.push(GremlinStep::BothV),
        "otherV" => out.push(GremlinStep::OtherV),
        "toE" => {
            // toE(Direction.OUT, "label") → outE("label"), etc.
            let raw_args = split_top_level_args(args_str);
            if !raw_args.is_empty() {
                let first = raw_args[0].trim();
                if first.starts_with("Direction.")
                    || first == "OUT"
                    || first == "IN"
                    || first == "BOTH"
                {
                    let labels: Vec<String> =
                        raw_args[1..].iter().map(|a| unquote(a.trim())).collect();
                    let labels: Vec<String> = labels
                        .into_iter()
                        .map(|l| {
                            if let Some(GremlinValue::String(s)) = resolve_param(&l) {
                                s
                            } else {
                                l
                            }
                        })
                        .collect();
                    match first {
                        "Direction.OUT" | "OUT" => out.push(GremlinStep::OutE(labels)),
                        "Direction.IN" | "IN" => out.push(GremlinStep::InE(labels)),
                        "Direction.BOTH" | "BOTH" => out.push(GremlinStep::BothE(labels)),
                        _ => out.push(GremlinStep::BothE(labels)),
                    }
                } else {
                    let args = parse_arg_list(args_str, resolve_param)?;
                    out.push(GremlinStep::ToE(args));
                }
            } else {
                let args = parse_arg_list(args_str, resolve_param)?;
                out.push(GremlinStep::ToE(args));
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}
