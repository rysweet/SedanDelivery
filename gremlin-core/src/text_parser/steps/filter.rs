//! Filter steps: has, hasLabel, hasId, hasNot, hasKey, hasValue, where, is, not,
//! and, or, filter, simplePath, cyclicPath, dedup, none, coin.

use super::super::*;
use super::super::{literal::*, predicate::*, tokenizer::*, traversal::*};

pub(super) fn try_dispatch(
    name: &str,
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<bool, TextParseError> {
    match name {
        "has" => parse_has_step(args_str, resolve_param, out)?,
        "hasLabel" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::HasLabel(args));
        }
        "hasId" => {
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::HasIdPredicate(pred));
            } else {
                let args = parse_arg_list(args_str, resolve_param)?;
                out.push(GremlinStep::HasId(args));
            }
        }
        "hasNot" => {
            let args = parse_string_arg_list(args_str)?;
            if let Some(key) = args.into_iter().next() {
                out.push(GremlinStep::Not(vec![GremlinStep::Has(key, None)]));
            }
        }
        "hasKey" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::HasKey(args));
        }
        "hasValue" => {
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::HasValuePredicate(pred));
            } else {
                let args = parse_arg_list(args_str, resolve_param)?;
                out.push(GremlinStep::HasValue(args));
            }
        }
        "is" => {
            let s = args_str.trim();
            let chain_parts = split_predicate_chain(s);
            if chain_parts.len() > 1 {
                let mut parsed: Vec<(String, GremlinStep)> = Vec::new();
                for (op, part) in &chain_parts {
                    if let Some(pred) = try_parse_predicate_call(part, resolve_param) {
                        parsed.push((op.to_string(), GremlinStep::Is(pred)));
                    }
                }
                let mut or_groups: Vec<Vec<GremlinStep>> = vec![vec![]];
                for (op, step) in parsed {
                    if op == "or" {
                        or_groups.push(vec![step]);
                    } else {
                        or_groups.last_mut().unwrap().push(step);
                    }
                }
                if or_groups.len() == 1 {
                    for step in or_groups.into_iter().flatten() {
                        out.push(step);
                    }
                } else {
                    let mut or_branches: Vec<Vec<GremlinStep>> = Vec::new();
                    for group in or_groups {
                        or_branches.push(group);
                    }
                    out.push(GremlinStep::Or(or_branches));
                }
            } else {
                let pred = parse_predicate(args_str, resolve_param)?;
                out.push(GremlinStep::Is(pred));
            }
        }
        "where" => {
            let raw_args = split_top_level_args(args_str);
            if raw_args.len() == 2 {
                let first = raw_args[0].trim();
                let second = raw_args[1].trim();
                let label = unquote(first);
                if let Some(pred) = try_parse_predicate_call(second, resolve_param) {
                    out.push(GremlinStep::Where(vec![
                        GremlinStep::Select(None, vec![label]),
                        GremlinStep::Is(pred),
                    ]));
                } else {
                    let inner_steps = parse_inner_traversal(args_str, resolve_param)?;
                    out.push(GremlinStep::Where(inner_steps));
                }
            } else {
                let inner_steps = parse_inner_traversal(args_str, resolve_param)?;
                out.push(GremlinStep::Where(inner_steps));
            }
        }
        "not" => {
            let inner_steps = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Not(inner_steps));
        }
        "and" => {
            let inner_traversals = parse_multi_traversal_args(args_str, resolve_param)?;
            out.push(GremlinStep::And(inner_traversals));
        }
        "or" => {
            let inner_traversals = parse_multi_traversal_args(args_str, resolve_param)?;
            out.push(GremlinStep::Or(inner_traversals));
        }
        "filter" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::FilterTraversal(inner));
        }
        "simplePath" => out.push(GremlinStep::SimplePath),
        "cyclicPath" => out.push(GremlinStep::CyclicPath),
        "dedup" => {
            if args_str.contains("Scope.local") || args_str.contains("local") {
                let args: Vec<String> = parse_string_arg_list(args_str)?
                    .into_iter()
                    .filter(|a| a != "Scope.local" && a != "local")
                    .collect();
                out.push(GremlinStep::DedupLocal(args));
            } else {
                let args = parse_string_arg_list(args_str)?;
                out.push(GremlinStep::Dedup(args));
            }
        }
        "none" => {
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::NonePred(pred));
            } else {
                out.push(GremlinStep::None);
            }
        }
        "coin" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let p = match args.first() {
                Some(GremlinValue::Float(f)) => *f,
                Some(GremlinValue::Integer(i)) => *i as f64,
                _ => 0.5,
            };
            out.push(GremlinStep::Coin(p));
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Parse a `has(...)` step into one or more [`GremlinStep`] values.
pub(crate) fn parse_has_step(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<(), TextParseError> {
    let raw_args = split_top_level_args(args_str);

    match raw_args.len() {
        0 => {
            return Err(TextParseError::InvalidArgument(
                "has() requires arguments".into(),
            ))
        }
        1 => {
            let key = parse_single_value(raw_args[0].trim(), resolve_param)?;
            match key {
                GremlinValue::String(k) => {
                    out.push(GremlinStep::Has(k, None));
                }
                GremlinValue::Null => {
                    out.push(GremlinStep::Is(Predicate::Eq(GremlinValue::Null)));
                }
                _ => {}
            }
        }
        2 => {
            let key = parse_single_value(raw_args[0].trim(), resolve_param)?;
            let val_str = raw_args[1].trim();

            if matches!(key, GremlinValue::Null) {
                out.push(GremlinStep::Is(Predicate::Eq(GremlinValue::Null)));
                return Ok(());
            }

            if let GremlinValue::String(ref k) = key {
                if k == "~label" {
                    if let Some(pred) = try_parse_predicate_call(val_str, resolve_param) {
                        if let Predicate::Eq(GremlinValue::String(label)) = pred {
                            out.push(GremlinStep::HasLabel(vec![label]));
                        } else {
                            out.push(GremlinStep::HasLabel(vec![]));
                        }
                    } else {
                        let val = parse_single_value(val_str, resolve_param)?;
                        if let GremlinValue::String(label) = val {
                            out.push(GremlinStep::HasLabel(vec![label]));
                        } else if matches!(val, GremlinValue::Null) {
                            out.push(GremlinStep::HasLabel(vec![
                                "__null_label_no_match__".to_string()
                            ]));
                        }
                    }
                    return Ok(());
                }
                if k == "~id" {
                    let val = parse_single_value(val_str, resolve_param)?;
                    out.push(GremlinStep::HasId(vec![val]));
                    return Ok(());
                }
            }

            if let Some(pred) = try_parse_predicate_call(val_str, resolve_param) {
                if let GremlinValue::String(k) = key {
                    out.push(GremlinStep::HasPredicate(k, pred));
                }
            } else {
                let val = parse_single_value(val_str, resolve_param)?;
                if let GremlinValue::String(k) = key {
                    out.push(GremlinStep::Has(k, Some(val)));
                }
            }
        }
        3 => {
            let label = parse_single_value(raw_args[0].trim(), resolve_param)?;
            let key = parse_single_value(raw_args[1].trim(), resolve_param)?;
            if let (GremlinValue::String(l), GremlinValue::String(k)) = (&label, &key) {
                out.push(GremlinStep::HasLabel(vec![l.clone()]));
                let val_str = raw_args[2].trim();
                if let Some(pred) = try_parse_predicate_call(val_str, resolve_param) {
                    out.push(GremlinStep::HasPredicate(k.clone(), pred));
                } else {
                    let val = parse_single_value(val_str, resolve_param)?;
                    out.push(GremlinStep::Has(k.clone(), Some(val)));
                }
            }
        }
        _ => {
            let key = parse_single_value(raw_args[0].trim(), resolve_param)?;
            if let GremlinValue::String(k) = key {
                out.push(GremlinStep::Has(k, None));
            }
        }
    }
    Ok(())
}
