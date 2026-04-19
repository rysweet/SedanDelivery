//! Limit / order steps: limit, range, skip, tail, sample, order, by, barrier.

use super::super::*;
use super::super::{literal::*, predicate::*, tokenizer::*};

pub(super) fn try_dispatch(
    name: &str,
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<bool, TextParseError> {
    match name {
        "order" => {
            if args_str.contains("Scope.local") || args_str.contains("local") {
                out.push(GremlinStep::SortLocal);
            } else {
                out.push(GremlinStep::Order);
            }
        }
        "by" => parse_by_step(args_str, out, resolve_param)?,
        "limit" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let is_local = args.iter().any(
                |a| matches!(a, GremlinValue::String(s) if s == "Scope.local" || s == "local"),
            );
            let n = args.iter().find_map(|a| {
                if let GremlinValue::Integer(v) = a {
                    Some(*v)
                } else {
                    None
                }
            });
            if let Some(n) = n {
                if is_local {
                    out.push(GremlinStep::LimitLocal(n));
                } else {
                    out.push(GremlinStep::Limit(n));
                }
            }
        }
        "range" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let is_local = args.iter().any(
                |a| matches!(a, GremlinValue::String(s) if s == "Scope.local" || s == "local"),
            );
            let ints: Vec<i64> = args
                .iter()
                .filter_map(|a| {
                    if let GremlinValue::Integer(v) = a {
                        Some(*v)
                    } else {
                        None
                    }
                })
                .collect();
            if ints.len() >= 2 {
                if is_local {
                    out.push(GremlinStep::RangeLocal(ints[0], ints[1]));
                } else {
                    out.push(GremlinStep::Range(ints[0], ints[1]));
                }
            }
        }
        "skip" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let is_local = args.iter().any(
                |a| matches!(a, GremlinValue::String(s) if s == "Scope.local" || s == "local"),
            );
            let n = args.iter().find_map(|a| {
                if let GremlinValue::Integer(v) = a {
                    Some(*v)
                } else {
                    None
                }
            });
            if let Some(n) = n {
                if is_local {
                    out.push(GremlinStep::SkipLocal(n));
                } else {
                    out.push(GremlinStep::Skip(n));
                }
            }
        }
        "tail" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let has_local_scope = args_str.contains("local") || args_str.contains("Scope.local");
            let n = args
                .iter()
                .find_map(|a| {
                    if let GremlinValue::Integer(v) = a {
                        Some(*v)
                    } else {
                        None
                    }
                })
                .unwrap_or(1);
            if has_local_scope {
                out.push(GremlinStep::TailLocal(n));
            } else {
                out.push(GremlinStep::Tail(n));
            }
        }
        "sample" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let is_local = args.iter().any(
                |a| matches!(a, GremlinValue::String(s) if s == "Scope.local" || s == "local"),
            );
            let n = args
                .iter()
                .find_map(|a| {
                    if let GremlinValue::Integer(v) = a {
                        Some(*v)
                    } else {
                        None
                    }
                })
                .unwrap_or(1);
            if is_local {
                out.push(GremlinStep::SampleLocal(n));
            } else {
                out.push(GremlinStep::Sample(n));
            }
        }
        "barrier" => {
            if args_str.contains("normSack") || args_str.contains("Barrier.normSack") {
                out.push(GremlinStep::BarrierNormSack);
            } else {
                out.push(GremlinStep::Barrier);
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Parse a `by(...)` modifier.
pub(crate) fn parse_by_step(
    args_str: &str,
    out: &mut Vec<GremlinStep>,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<(), TextParseError> {
    let raw_args = split_top_level_args(args_str);

    if raw_args.is_empty() {
        out.push(GremlinStep::By(String::new(), false));
        return Ok(());
    }

    let first = raw_args[0].trim();

    if first == "Order.desc" || first == "desc" {
        out.push(GremlinStep::By(String::new(), true));
        return Ok(());
    }
    if first == "Order.asc" || first == "asc" {
        out.push(GremlinStep::By(String::new(), false));
        return Ok(());
    }
    if first == "Order.shuffle" || first == "shuffle" {
        out.push(GremlinStep::By(String::new(), false));
        return Ok(());
    }

    let descending_when_second = || {
        raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        }
    };

    if first == "T.label" || first == "t[label]" {
        out.push(GremlinStep::By(
            "~label".to_string(),
            descending_when_second(),
        ));
        return Ok(());
    }
    if first == "T.id" || first == "t[id]" {
        out.push(GremlinStep::By("~id".to_string(), descending_when_second()));
        return Ok(());
    }
    if first == "T.key" || first == "t[key]" {
        out.push(GremlinStep::By(
            "~key".to_string(),
            descending_when_second(),
        ));
        return Ok(());
    }
    if first == "T.value" || first == "t[value]" {
        out.push(GremlinStep::By(
            "~value".to_string(),
            descending_when_second(),
        ));
        return Ok(());
    }
    if first == "Column.keys" || first == "Column.values" {
        out.push(GremlinStep::By(first.to_string(), descending_when_second()));
        return Ok(());
    }

    if first == "label" || first == "Label" {
        out.push(GremlinStep::By(
            "~label".to_string(),
            descending_when_second(),
        ));
        return Ok(());
    }
    if first == "id" || first == "Id" {
        out.push(GremlinStep::By("~id".to_string(), descending_when_second()));
        return Ok(());
    }
    if first == "key" || first == "Key" {
        out.push(GremlinStep::By(
            "~key".to_string(),
            descending_when_second(),
        ));
        return Ok(());
    }
    if first == "value" || first == "Value" {
        out.push(GremlinStep::By(
            "~value".to_string(),
            descending_when_second(),
        ));
        return Ok(());
    }

    let descending = descending_when_second();

    if looks_like_traversal(first) {
        let inner = parse_inner_traversal(first, resolve_param)?;
        if !inner.is_empty() {
            out.push(GremlinStep::ByTraversal(inner, descending));
            return Ok(());
        }
    }

    let first_unquoted = unquote(first);
    out.push(GremlinStep::By(first_unquoted, descending));
    Ok(())
}
