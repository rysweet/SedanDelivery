//! Source / entry-point steps: V, E, addV, addE, inject.

use super::super::literal::*;
use super::super::*;

pub(super) fn try_dispatch(
    name: &str,
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<bool, TextParseError> {
    match name {
        "V" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if args.is_empty() {
                out.push(GremlinStep::V(None));
            } else {
                out.push(GremlinStep::V(Some(args)));
            }
        }
        "E" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if args.is_empty() {
                out.push(GremlinStep::E(None));
            } else {
                out.push(GremlinStep::E(Some(args)));
            }
        }
        "addV" => {
            if args_str.contains('(') {
                let inner = parse_inner_traversal(args_str, resolve_param)?;
                if !inner.is_empty() {
                    out.push(GremlinStep::AddVTraversal(inner));
                } else {
                    let args = parse_string_arg_list(args_str)?;
                    let label = args.into_iter().next().unwrap_or_default();
                    out.push(GremlinStep::AddV(label));
                }
            } else {
                let args = parse_string_arg_list(args_str)?;
                let label = args.into_iter().next().unwrap_or_default();
                out.push(GremlinStep::AddV(label));
            }
        }
        "addE" => {
            let args = parse_string_arg_list(args_str)?;
            let label = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::AddE(label));
        }
        "inject" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            out.push(GremlinStep::Inject(args));
        }
        _ => return Ok(false),
    }
    Ok(true)
}
