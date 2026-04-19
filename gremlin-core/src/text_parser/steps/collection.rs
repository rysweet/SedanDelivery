//! Collection / quantifier steps: combine, intersect, difference, disjunct,
//! merge (list), product, all, any.

use super::super::*;
use super::super::{literal::*, predicate::*};

pub(super) fn try_dispatch(
    name: &str,
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<bool, TextParseError> {
    match name {
        "combine" => {
            let inner = parse_collection_step_arg(args_str, resolve_param)?;
            out.push(GremlinStep::Combine(inner));
        }
        "intersect" => {
            let inner = parse_collection_step_arg(args_str, resolve_param)?;
            out.push(GremlinStep::Intersect(inner));
        }
        "difference" => {
            let inner = parse_collection_step_arg(args_str, resolve_param)?;
            out.push(GremlinStep::Difference(inner));
        }
        "disjunct" => {
            let inner = parse_collection_step_arg(args_str, resolve_param)?;
            out.push(GremlinStep::Disjunct(inner));
        }
        "merge" => {
            let inner = parse_collection_step_arg(args_str, resolve_param)?;
            out.push(GremlinStep::MergeList(inner));
        }
        "product" => {
            let inner = parse_collection_step_arg(args_str, resolve_param)?;
            out.push(GremlinStep::Product(inner));
        }
        "all" => {
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::All(pred));
            } else {
                out.push(GremlinStep::All(Predicate::Neq(GremlinValue::Null)));
            }
        }
        "any" => {
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::Any(pred));
            } else {
                out.push(GremlinStep::Any(Predicate::Neq(GremlinValue::Null)));
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}
