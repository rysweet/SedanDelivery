//! Path-related steps: path, from, to.

use super::super::traversal::*;
use super::super::*;

pub(super) fn try_dispatch(
    name: &str,
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<bool, TextParseError> {
    match name {
        "path" => out.push(GremlinStep::Path),
        "from" => {
            let inner = parse_from_to_traversal(args_str, resolve_param)?;
            if inner.len() == 1
                && matches!(
                    &inner[0],
                    GremlinStep::Out(_) | GremlinStep::In(_) | GremlinStep::Both(_)
                )
            {
                out.extend(inner);
            } else {
                out.push(GremlinStep::From(inner));
            }
        }
        "to" => {
            let inner = parse_from_to_traversal(args_str, resolve_param)?;
            if inner.len() == 1
                && matches!(
                    &inner[0],
                    GremlinStep::Out(_) | GremlinStep::In(_) | GremlinStep::Both(_)
                )
            {
                out.extend(inner);
            } else {
                out.push(GremlinStep::To(inner));
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}
