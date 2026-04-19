//! Aggregation steps: count, sum, min, max, mean, fold, unfold, group, groupCount,
//! tree, subgraph, cap, aggregate, store.

use super::super::literal::*;
use super::super::*;

pub(super) fn try_dispatch(
    name: &str,
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<bool, TextParseError> {
    match name {
        "count" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_count_local".into()));
            } else {
                out.push(GremlinStep::Count);
            }
        }
        "sum" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::SumLocal);
            } else {
                out.push(GremlinStep::Sum);
            }
        }
        "min" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::MinLocal);
            } else {
                out.push(GremlinStep::Min);
            }
        }
        "max" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::MaxLocal);
            } else {
                out.push(GremlinStep::Max);
            }
        }
        "mean" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::MeanLocal);
            } else {
                out.push(GremlinStep::Mean);
            }
        }
        "group" => {
            let args = parse_string_arg_list(args_str)?;
            let key = args.into_iter().next();
            out.push(GremlinStep::Group(key));
        }
        "groupCount" => {
            let args = parse_string_arg_list(args_str)?;
            let key = args.into_iter().next();
            out.push(GremlinStep::GroupCount(key));
        }
        "fold" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if args.len() >= 2 {
                let seed = args[0].clone();
                let op_str = match &args[1] {
                    GremlinValue::String(s) => s.clone(),
                    _ => "sum".to_string(),
                };
                out.push(GremlinStep::FoldSeed(seed, op_str));
            } else {
                out.push(GremlinStep::Fold);
            }
        }
        "unfold" => out.push(GremlinStep::Unfold),
        "tree" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Tree(args.into_iter().next()));
        }
        "subgraph" => {
            let args = parse_string_arg_list(args_str)?;
            let key = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::Subgraph(key));
        }
        "aggregate" => {
            let args = parse_string_arg_list(args_str)?;
            let mut iter = args.into_iter();
            if let Some(first) = iter.next() {
                if first == "local"
                    || first == "Scope.local"
                    || first == "global"
                    || first == "Scope.global"
                {
                    if let Some(key) = iter.next() {
                        out.push(GremlinStep::Aggregate(key));
                    }
                } else {
                    out.push(GremlinStep::Aggregate(first));
                }
            }
        }
        "cap" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Cap(args));
        }
        "store" => {
            let args = parse_string_arg_list(args_str)?;
            if let Some(key) = args.into_iter().next() {
                out.push(GremlinStep::Aggregate(key));
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}
