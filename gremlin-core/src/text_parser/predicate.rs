//! Predicate (`P.eq`, `P.gt`, ...) parsing and traversal-vs-value disambiguation.

use super::*;
use super::{literal::*, tokenizer::*};

pub(crate) fn looks_like_traversal(s: &str) -> bool {
    let s = s.trim();
    // Quoted strings are property names, not traversals
    if s.starts_with('"') || s.starts_with('\'') {
        return false;
    }
    // Strip optional __.prefix for detection
    let check = s.strip_prefix("__.").unwrap_or(s);
    const TRAVERSAL_STEP_PREFIXES: &[&str] = &[
        "values(",
        "out(",
        "in(",
        "both(",
        "outE(",
        "inE(",
        "bothE(",
        "label(",
        "id(",
        "count(",
        "sum(",
        "min(",
        "max(",
        "mean(",
        "select(",
        "has(",
        "hasLabel(",
        "hasId(",
        "key(",
        "value(",
        "constant(",
        "identity(",
        "loops(",
        "otherV(",
        "inV(",
        "outV(",
        "bothV(",
        "properties(",
        "valueMap(",
        "elementMap(",
        "tail(",
        "unfold(",
    ];
    for step in TRAVERSAL_STEP_PREFIXES {
        if check.starts_with(step) {
            let after_name = &check[step.len() - 1..];
            if let Some(close_pos) = find_matching_paren(after_name) {
                let after_close = after_name[close_pos + 1..].trim();
                if after_close.is_empty() {
                    return true;
                }
                // Chained traversal: `.count()`, `.values("name")`, etc.
                if after_close.starts_with('.') {
                    return true;
                }
            }
        }
    }
    false
}

/// Find the position of the matching closing parenthesis.
pub(crate) fn try_parse_predicate_call(
    s: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Option<Predicate> {
    let s = s.trim();

    // Check for predicate chains: P.gt(18).and(P.lt(30)).or(P.gt(35))
    let chain_parts = split_predicate_chain(s);
    if chain_parts.len() > 1 {
        // Build compound predicate from chain
        let mut or_groups: Vec<Vec<Predicate>> = vec![vec![]];
        for (op, part) in &chain_parts {
            let pred = try_parse_predicate_call(part, resolve_param)?;
            if *op == "or" {
                or_groups.push(vec![pred]);
            } else {
                or_groups.last_mut().unwrap().push(pred);
            }
        }
        if or_groups.len() == 1 {
            // All AND — return first compound as And
            let preds = or_groups.into_iter().next().unwrap();
            if preds.len() == 1 {
                return preds.into_iter().next();
            }
            return Some(Predicate::And(preds));
        }
        // OR of AND groups
        let mut or_preds = Vec::new();
        for group in or_groups {
            if group.len() == 1 {
                or_preds.push(group.into_iter().next().unwrap());
            } else {
                or_preds.push(Predicate::And(group));
            }
        }
        return Some(Predicate::Or(or_preds));
    }

    try_parse_predicate_single(s, resolve_param)
}

pub(crate) fn try_parse_predicate_single(
    s: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Option<Predicate> {
    let s = s.trim();

    // Handle P.* or TextP.* predicates
    let pred_str = s.strip_prefix("P.").or_else(|| s.strip_prefix("TextP."));
    let pred_str = if let Some(p) = pred_str {
        p
    } else {
        // Try bare predicate names: eq(...), neq(...), gt(...), etc.
        let (name, _) = split_step_name_args(s);
        match name {
            "eq" | "neq" | "gt" | "gte" | "lt" | "lte" | "within" | "without" | "between"
            | "inside" | "containing" | "notContaining" | "startingWith" | "notStartingWith"
            | "endingWith" | "notEndingWith" | "regex" | "notRegex" | "typeOf" => s,
            _ => return None,
        }
    };

    let (pred_name, args_str) = split_step_name_args(pred_str);
    let args = parse_arg_list(args_str, resolve_param).ok()?;

    match pred_name {
        "eq" => Some(Predicate::Eq(args.into_iter().next()?)),
        "neq" => Some(Predicate::Neq(args.into_iter().next()?)),
        "gt" => Some(Predicate::Gt(args.into_iter().next()?)),
        "gte" => Some(Predicate::Gte(args.into_iter().next()?)),
        "lt" => Some(Predicate::Lt(args.into_iter().next()?)),
        "lte" => Some(Predicate::Lte(args.into_iter().next()?)),
        "within" => Some(Predicate::Within(args)),
        "without" => Some(Predicate::Without(args)),
        "between" => {
            let mut it = args.into_iter();
            let lo = it.next()?;
            let hi = it.next()?;
            Some(Predicate::Between(lo, hi))
        }
        "inside" => {
            let mut it = args.into_iter();
            let lo = it.next()?;
            let hi = it.next()?;
            Some(Predicate::Inside(lo, hi))
        }
        // Text predicates
        "containing" => Some(Predicate::Containing(args.into_iter().next()?)),
        "notContaining" | "notcontaining" => {
            Some(Predicate::NotContaining(args.into_iter().next()?))
        }
        "startingWith" | "startingwith" => Some(Predicate::StartingWith(args.into_iter().next()?)),
        "notStartingWith" | "notstartingwith" => {
            Some(Predicate::NotStartingWith(args.into_iter().next()?))
        }
        "endingWith" | "endingwith" => Some(Predicate::EndingWith(args.into_iter().next()?)),
        "notEndingWith" | "notendingwith" => {
            Some(Predicate::NotEndingWith(args.into_iter().next()?))
        }
        "regex" => Some(Predicate::Regex(args.into_iter().next()?)),
        "notRegex" | "notregex" => Some(Predicate::NotRegex(args.into_iter().next()?)),
        "typeOf" | "typeof" => {
            // P.typeOf(GType.STRING) → IsType("string")
            let arg = args.into_iter().next()?;
            // GType.XXX is parsed as a string, normalize it
            let type_name = match &arg {
                GremlinValue::String(s) => {
                    let s = s.strip_prefix("GType.").unwrap_or(s);
                    normalize_gtype(s)
                }
                _ => return None,
            };
            Some(Predicate::IsType(GremlinValue::String(type_name)))
        }
        "not" => {
            // P.not(P.eq(1)) — negate inner predicate
            // The arg string is the inner predicate call
            let inner = try_parse_predicate_call(args_str, resolve_param)?;
            Some(Predicate::Not(Box::new(inner)))
        }
        _ => None,
    }
}

/// Split a predicate chain like `P.eq(1).and(P.gt(0)).or(P.lt(5))` into parts.
/// Returns vec of (op, predicate_str) where op is "first", "and", or "or".
pub(crate) fn parse_predicate(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Predicate, TextParseError> {
    let s = args_str.trim();

    // Try P.* predicate
    if let Some(pred) = try_parse_predicate_call(s, resolve_param) {
        return Ok(pred);
    }

    // Plain value: is(42)
    let val = parse_single_value(s, resolve_param)?;
    Ok(Predicate::Eq(val))
}
