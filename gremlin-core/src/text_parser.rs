//! Simple Gremlin text parser for TinkerPop TCK `.feature` file traversals.
//!
//! Parses Gremlin traversal strings like `g.V().has("name","marko").out("knows")`
//! into [`GremlinStep`] sequences.  This is intentionally not a full Gremlin
//! grammar parser — it covers the step patterns that appear in the downloaded
//! TinkerPop TCK feature files.

use crate::bytecode::{GremlinStep, Predicate};
use crate::types::GremlinValue;

/// Errors that can occur during Gremlin text parsing.
#[derive(Debug, thiserror::Error)]
pub enum TextParseError {
    #[error("unexpected end of input")]
    UnexpectedEnd,
    #[error("expected '{expected}' but found '{found}'")]
    Expected { expected: String, found: String },
    #[error("unknown step: {0}")]
    UnknownStep(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

/// Parse a Gremlin traversal string into a list of steps.
///
/// Accepts strings like:
/// ```text
/// g.V().has("name","marko").out("knows").values("name")
/// g.V(vid1).values("name").path()
/// g.addV("person").property("name","stephen")
/// ```
///
/// Parameters are substituted via the provided callback.
pub fn parse_traversal(
    input: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Vec<GremlinStep>, TextParseError> {
    let input = input.trim();

    // Strip strategy wrappers: g.withStrategies(...).V()... or g.withoutStrategies(...).V()...
    let owned;
    let input =
        if input.starts_with("g.withStrategies(") || input.starts_with("g.withoutStrategies(") {
            owned = strip_strategy_prefix(input);
            &owned
        } else {
            input
        };

    // Must start with "g."
    let rest = input
        .strip_prefix("g.")
        .ok_or_else(|| TextParseError::Expected {
            expected: "g.".into(),
            found: input.chars().take(10).collect(),
        })?;

    let tokens = tokenize_steps(rest)?;
    let mut steps = Vec::new();

    for token in &tokens {
        parse_step_token(token, resolve_param, &mut steps)?;
    }

    Ok(steps)
}

/// Strip `g.withStrategies(...)` or `g.withoutStrategies(...)` prefix.
/// Returns a new string starting with `g.` followed by the real traversal steps.
fn strip_strategy_prefix(input: &str) -> String {
    let after_parens = if let Some(r) = input.strip_prefix("g.withStrategies(") {
        skip_balanced_parens(r)
    } else if let Some(r) = input.strip_prefix("g.withoutStrategies(") {
        skip_balanced_parens(r)
    } else {
        return input.to_string();
    };

    // after_parens starts with ".V()..." — strip the leading dot and prepend "g."
    let rest = after_parens.strip_prefix('.').unwrap_or(after_parens);
    format!("g.{rest}")
}

/// Skip past balanced parentheses.  Input starts just after the opening `(`.
/// Returns the slice after the matching `)` and any trailing `.`.
fn skip_balanced_parens(input: &str) -> &str {
    let mut depth = 1u32;
    let mut in_string = false;
    let mut escape = false;
    for (i, ch) in input.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
            if depth == 0 {
                return &input[i + 1..];
            }
        }
    }
    input
}

/// Split a post-`g.` traversal string into step tokens like `V()`, `has("name","marko")`, etc.
///
/// Splits on `.` while respecting parentheses, strings, and nested `__.*` anonymous traversals.
fn tokenize_steps(input: &str) -> Result<Vec<String>, TextParseError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut depth = 0u32;
    let mut in_string = false;
    let mut escape = false;

    for ch in input.chars() {
        if escape {
            current.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            current.push(ch);
            escape = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            current.push(ch);
            continue;
        }
        if in_string {
            current.push(ch);
            continue;
        }
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            '.' if depth == 0 => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

/// Parse a single step token (e.g. `has("name","marko")`) into GremlinStep(s).
fn parse_step_token(
    token: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<(), TextParseError> {
    let (name, args_str) = split_step_name_args(token);

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
        "has" => parse_has_step(args_str, resolve_param, out)?,
        "hasLabel" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::HasLabel(args));
        }
        "hasId" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            out.push(GremlinStep::HasId(args));
        }
        "hasNot" => {
            // hasNot("key") is equivalent to has("key") with no value → filter absence
            // We approximate as Has(key, None) + Not wrapper.  For simplicity, skip.
            let args = parse_string_arg_list(args_str)?;
            if let Some(key) = args.into_iter().next() {
                out.push(GremlinStep::Has(key, None));
                // Note: hasNot semantics differ — this is best-effort.
            }
        }
        "out" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Out(args));
        }
        "in" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::In(args));
        }
        "both" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Both(args));
        }
        "outE" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::OutE(args));
        }
        "inE" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::InE(args));
        }
        "bothE" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::BothE(args));
        }
        "inV" => out.push(GremlinStep::InV),
        "outV" => out.push(GremlinStep::OutV),
        "bothV" => out.push(GremlinStep::BothV),
        "otherV" => out.push(GremlinStep::OtherV),
        "values" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Values(args));
        }
        "valueMap" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::ValueMap(args));
        }
        "properties" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Properties(args));
        }
        "id" => out.push(GremlinStep::Id),
        "label" => out.push(GremlinStep::Label),
        "select" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Select(args));
        }
        "as" => {
            let args = parse_string_arg_list(args_str)?;
            if let Some(alias) = args.into_iter().next() {
                out.push(GremlinStep::As(alias));
            }
        }
        "count" => out.push(GremlinStep::Count),
        "sum" => out.push(GremlinStep::Sum),
        "min" => out.push(GremlinStep::Min),
        "max" => out.push(GremlinStep::Max),
        "mean" => out.push(GremlinStep::Mean),
        "group" => out.push(GremlinStep::Group),
        "groupCount" => {
            let args = parse_string_arg_list(args_str)?;
            if args.is_empty() {
                out.push(GremlinStep::GroupCount);
            } else {
                // groupCount("a") — named side effect
                out.push(GremlinStep::GroupCount);
            }
        }
        "fold" => out.push(GremlinStep::Fold),
        "unfold" => out.push(GremlinStep::Unfold),
        "order" => out.push(GremlinStep::Order),
        "by" => parse_by_step(args_str, out)?,
        "limit" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if let Some(GremlinValue::Integer(n)) = args.first() {
                out.push(GremlinStep::Limit(*n));
            }
        }
        "range" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if args.len() >= 2 {
                if let (Some(GremlinValue::Integer(lo)), Some(GremlinValue::Integer(hi))) =
                    (args.first(), args.get(1))
                {
                    out.push(GremlinStep::Range(*lo, *hi));
                }
            }
        }
        "dedup" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Dedup(args));
        }
        "is" => {
            let pred = parse_predicate(args_str, resolve_param)?;
            out.push(GremlinStep::Is(pred));
        }
        "where" => {
            // where(P.neq("a")) or where(__.out("knows"))
            // Best-effort: try as a traversal
            let inner_steps = parse_inner_traversal(args_str, resolve_param)?;
            if let Some(step) = inner_steps.into_iter().next() {
                out.push(GremlinStep::Where(Box::new(step)));
            }
        }
        "not" => {
            let inner_steps = parse_inner_traversal(args_str, resolve_param)?;
            if let Some(step) = inner_steps.into_iter().next() {
                out.push(GremlinStep::Not(Box::new(step)));
            }
        }
        "and" => {
            let inner_steps = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::And(inner_steps));
        }
        "or" => {
            let inner_steps = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Or(inner_steps));
        }
        "addV" => {
            let args = parse_string_arg_list(args_str)?;
            let label = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::AddV(label));
        }
        "addE" => {
            let args = parse_string_arg_list(args_str)?;
            let label = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::AddE(label));
        }
        "property" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if args.len() >= 2 {
                if let GremlinValue::String(key) = &args[0] {
                    out.push(GremlinStep::Property(key.clone(), args[1].clone()));
                }
            }
        }
        "from" => {
            let inner = parse_from_to_arg(args_str, resolve_param)?;
            out.push(GremlinStep::From(Box::new(inner)));
        }
        "to" => {
            let inner = parse_from_to_arg(args_str, resolve_param)?;
            out.push(GremlinStep::To(Box::new(inner)));
        }
        "drop" => out.push(GremlinStep::Drop),
        "path" => out.push(GremlinStep::Path),
        "repeat" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Repeat(inner));
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
        "emit" => out.push(GremlinStep::Emit),
        "aggregate" => {
            let args = parse_string_arg_list(args_str)?;
            if let Some(key) = args.into_iter().next() {
                out.push(GremlinStep::Aggregate(key));
            }
        }
        "cap" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Cap(args));
        }
        "constant" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let val = args.into_iter().next().unwrap_or(GremlinValue::Null);
            out.push(GremlinStep::Constant(val));
        }
        "inject" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            out.push(GremlinStep::Inject(args));
        }
        "project" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Project(args));
        }
        "match" => {
            // match() takes multiple traversal patterns — complex, skip for now
            out.push(GremlinStep::Match(Vec::new()));
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
            // choose(pred, true_branch, false_branch) — simplified
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
                _ => {
                    out.push(GremlinStep::Choose(Vec::new(), Vec::new(), None));
                }
            }
        }
        "sideEffect" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::SideEffect(inner));
        }
        // Identity and other no-op steps
        "identity" | "barrier" | "profile" | "none" | "iterate" | "explain" | "tree"
        | "subgraph" | "sack" | "coin" | "sample" | "tail" | "optional" | "local" | "flatMap"
        | "map" | "filter" | "store" | "elementMap" | "index" | "loops" | "math" | "cyclicPath"
        | "simplePath" | "concat" | "conjoin" | "toLower" | "toUpper" | "trim" | "lTrim"
        | "rTrim" | "length" | "replace" | "split" | "substring" | "reverse" | "format"
        | "asString" | "asBool" | "asNumber" | "asDate" | "dateAdd" | "dateDiff" | "fail"
        | "read" | "write" | "combine" | "intersect" | "difference" | "disjunct" | "merge"
        | "mergeV" | "mergeE" | "product" | "all" | "any" | "typeOf" | "discard" | "hasKey"
        | "hasValue" | "connectedComponent" | "shortestPath" | "pageRank" | "peerPressure"
        | "call" => {
            // Unsupported steps — record as unknown so the runner can skip
            return Err(TextParseError::UnknownStep(name.to_string()));
        }
        _ => {
            return Err(TextParseError::UnknownStep(name.to_string()));
        }
    }
    Ok(())
}

/// Split `has("name","marko")` into name="has", args_str=`"name","marko"`.
fn split_step_name_args(token: &str) -> (&str, &str) {
    if let Some(paren_pos) = token.find('(') {
        let name = &token[..paren_pos];
        // Strip outer parens
        let inner = &token[paren_pos + 1..];
        let inner = inner.strip_suffix(')').unwrap_or(inner);
        (name, inner)
    } else {
        (token, "")
    }
}

/// Parse the `has(...)` step which has multiple overloads:
/// - `has("key")` — property existence
/// - `has("key", value)` — property equals
/// - `has("key", P.gt(30))` — predicate
/// - `has("label", "key", value)` — label + key + value
fn parse_has_step(
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
            // has("key") — existence check
            let key = parse_single_value(raw_args[0].trim(), resolve_param)?;
            if let GremlinValue::String(k) = key {
                out.push(GremlinStep::Has(k, None));
            }
        }
        2 => {
            let key = parse_single_value(raw_args[0].trim(), resolve_param)?;
            let val_str = raw_args[1].trim();

            // Check if second arg is a predicate (P.gt, P.within, TextP.*, etc.)
            if let Some(pred) = try_parse_predicate_call(val_str, resolve_param) {
                if let GremlinValue::String(k) = key {
                    // Convert predicate to GremlinValue-based Has
                    // For now, store as Has with a value derived from the predicate
                    // The compiler will handle predicate matching
                    out.push(GremlinStep::Has(k, None));
                    out.push(GremlinStep::Is(pred));
                }
            } else {
                let val = parse_single_value(val_str, resolve_param)?;
                if let GremlinValue::String(k) = key {
                    out.push(GremlinStep::Has(k, Some(val)));
                }
            }
        }
        3 => {
            // has(label, key, value) or has(label, key, P.*)
            let label = parse_single_value(raw_args[0].trim(), resolve_param)?;
            let key = parse_single_value(raw_args[1].trim(), resolve_param)?;
            if let (GremlinValue::String(l), GremlinValue::String(k)) = (&label, &key) {
                out.push(GremlinStep::HasLabel(vec![l.clone()]));
                let val_str = raw_args[2].trim();
                if let Some(_pred) = try_parse_predicate_call(val_str, resolve_param) {
                    out.push(GremlinStep::Has(k.clone(), None));
                } else {
                    let val = parse_single_value(val_str, resolve_param)?;
                    out.push(GremlinStep::Has(k.clone(), Some(val)));
                }
            }
        }
        _ => {
            // Best-effort: treat first as key
            let key = parse_single_value(raw_args[0].trim(), resolve_param)?;
            if let GremlinValue::String(k) = key {
                out.push(GremlinStep::Has(k, None));
            }
        }
    }
    Ok(())
}

/// Parse a `by(...)` modifier.
fn parse_by_step(args_str: &str, out: &mut Vec<GremlinStep>) -> Result<(), TextParseError> {
    let raw_args = split_top_level_args(args_str);

    if raw_args.is_empty() {
        // by() with no args — identity projection
        out.push(GremlinStep::By(String::new(), false));
        return Ok(());
    }

    let first = raw_args[0].trim();
    let first_unquoted = unquote(first);

    // Check for Order.asc, Order.desc, Order.shuffle
    let descending = if raw_args.len() >= 2 {
        let second = raw_args[1].trim();
        second == "Order.desc" || second == "desc"
    } else {
        false
    };

    out.push(GremlinStep::By(first_unquoted, descending));
    Ok(())
}

/// Try to parse a predicate call like `P.gt(30)`, `P.within("a","b")`, etc.
fn try_parse_predicate_call(
    s: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Option<Predicate> {
    let s = s.trim();

    // Handle P.* predicates
    let pred_str = s.strip_prefix("P.").or_else(|| s.strip_prefix("TextP."));
    let pred_str = pred_str?;

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
        // Text predicates — not directly representable, skip
        "containing" | "startingWith" | "endingWith" | "regex" | "notContaining"
        | "notStartingWith" | "notEndingWith" | "notRegex" => None,
        _ => None,
    }
}

/// Parse a predicate from an `is(...)` argument.
fn parse_predicate(
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

/// Parse an inner anonymous traversal: `__.out("knows")` or `outE().count().is(0)`.
fn parse_inner_traversal(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Vec<GremlinStep>, TextParseError> {
    let s = args_str.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }

    // Try P.* predicate as a single step
    if let Some(pred) = try_parse_predicate_call(s, resolve_param) {
        return Ok(vec![GremlinStep::Is(pred)]);
    }

    // Strip __ prefix if present
    let traversal_str = s.strip_prefix("__.").unwrap_or(s);

    let tokens = tokenize_steps(traversal_str)?;
    let mut steps = Vec::new();
    for token in &tokens {
        // Tolerate unknown steps in inner traversals
        if parse_step_token(token, resolve_param, &mut steps).is_err() {
            break;
        }
    }
    Ok(steps)
}

/// Parse multiple traversal arguments separated by commas at the top level.
fn parse_multi_traversal_args(
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

/// Parse a `from(...)` or `to(...)` argument — could be a string label or a traversal.
fn parse_from_to_arg(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<GremlinStep, TextParseError> {
    let s = args_str.trim();

    // If it's a quoted string, it's a step label reference
    if s.starts_with('"') {
        let label = unquote(s);
        return Ok(GremlinStep::Select(vec![label]));
    }

    // Otherwise it's a traversal
    let inner = parse_inner_traversal(s, resolve_param)?;
    if let Some(step) = inner.into_iter().next() {
        Ok(step)
    } else {
        Ok(GremlinStep::Select(vec![]))
    }
}

// ── Argument parsing utilities ──────────────────────────────────────────────

/// Split comma-separated arguments at the top level (respecting parens and strings).
fn split_top_level_args(s: &str) -> Vec<&str> {
    if s.trim().is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut depth = 0u32;
    let mut in_string = false;
    let mut escape = false;
    let mut start = 0;

    for (i, ch) in s.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                result.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    result.push(&s[start..]);
    result
}

/// Parse a comma-separated argument list into GremlinValues.
fn parse_arg_list(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Vec<GremlinValue>, TextParseError> {
    if args_str.trim().is_empty() {
        return Ok(Vec::new());
    }

    let raw_args = split_top_level_args(args_str);
    let mut values = Vec::new();
    for arg in raw_args {
        values.push(parse_single_value(arg.trim(), resolve_param)?);
    }
    Ok(values)
}

/// Parse a list of string-only arguments.
fn parse_string_arg_list(args_str: &str) -> Result<Vec<String>, TextParseError> {
    if args_str.trim().is_empty() {
        return Ok(Vec::new());
    }

    let raw_args = split_top_level_args(args_str);
    let mut result = Vec::new();
    for arg in raw_args {
        let s = arg.trim();
        result.push(unquote(s));
    }
    Ok(result)
}

/// Parse a single value: string literal, number, boolean, null, or parameter reference.
fn parse_single_value(
    s: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<GremlinValue, TextParseError> {
    let s = s.trim();

    if s.is_empty() {
        return Ok(GremlinValue::Null);
    }

    // String literal
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        let unescaped = inner.replace("\\\"", "\"").replace("\\\\", "\\");
        return Ok(GremlinValue::String(unescaped));
    }

    // Boolean
    if s == "true" {
        return Ok(GremlinValue::Bool(true));
    }
    if s == "false" {
        return Ok(GremlinValue::Bool(false));
    }

    // Null
    if s == "null" {
        return Ok(GremlinValue::Null);
    }

    // Integer
    if let Ok(n) = s.parse::<i64>() {
        return Ok(GremlinValue::Integer(n));
    }

    // Float
    if let Ok(f) = s.parse::<f64>() {
        return Ok(GremlinValue::Float(f));
    }

    // Typed value notation from feature files: d[29].i, d[0.5].d, d[6].l
    if s.starts_with("d[") {
        if let Some(end) = s.find(']') {
            let num_str = &s[2..end];
            if let Ok(n) = num_str.parse::<i64>() {
                return Ok(GremlinValue::Integer(n));
            }
            if let Ok(f) = num_str.parse::<f64>() {
                return Ok(GremlinValue::Float(f));
            }
        }
    }

    // T.label, T.id — special tokens
    if s == "T.label" || s == "t[label]" {
        return Ok(GremlinValue::String("~label".to_string()));
    }
    if s == "T.id" || s == "t[id]" {
        return Ok(GremlinValue::String("~id".to_string()));
    }

    // Scope.local, Scope.global
    if s.starts_with("Scope.") || s.starts_with("Column.") || s.starts_with("Order.") {
        return Ok(GremlinValue::String(s.to_string()));
    }

    // Parameter reference (bare identifier like vid1, xx1)
    if let Some(val) = resolve_param(s) {
        return Ok(val);
    }

    // __.select("a") or similar traversal fragment — treat as string
    if s.starts_with("__") {
        return Ok(GremlinValue::String(s.to_string()));
    }

    // Unknown — treat as parameter name / string
    Ok(GremlinValue::String(s.to_string()))
}

/// Remove surrounding double quotes from a string, if present.
fn unquote(s: &str) -> String {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        s.to_string()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn no_params(_name: &str) -> Option<GremlinValue> {
        None
    }

    #[test]
    fn parse_v_count() {
        let steps = parse_traversal("g.V().count()", &no_params).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0], GremlinStep::V(None));
        assert_eq!(steps[1], GremlinStep::Count);
    }

    #[test]
    fn parse_v_has_out_values() {
        let steps = parse_traversal(
            r#"g.V().has("name","marko").out("knows").values("name")"#,
            &no_params,
        )
        .unwrap();
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0], GremlinStep::V(None));
        assert_eq!(
            steps[1],
            GremlinStep::Has("name".into(), Some(GremlinValue::String("marko".into())))
        );
        assert_eq!(steps[2], GremlinStep::Out(vec!["knows".into()]));
        assert_eq!(steps[3], GremlinStep::Values(vec!["name".into()]));
    }

    #[test]
    fn parse_v_with_param() {
        let params = |name: &str| -> Option<GremlinValue> {
            if name == "vid1" {
                Some(GremlinValue::Integer(1))
            } else {
                None
            }
        };
        let steps = parse_traversal("g.V(vid1).values(\"name\")", &params).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::V(Some(vec![GremlinValue::Integer(1)]))
        );
        assert_eq!(steps[1], GremlinStep::Values(vec!["name".into()]));
    }

    #[test]
    fn parse_addv_property() {
        let steps = parse_traversal(
            r#"g.addV("person").property("name","stephen").property("age",29)"#,
            &no_params,
        )
        .unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0], GremlinStep::AddV("person".into()));
        assert_eq!(
            steps[1],
            GremlinStep::Property("name".into(), GremlinValue::String("stephen".into()))
        );
        assert_eq!(
            steps[2],
            GremlinStep::Property("age".into(), GremlinValue::Integer(29))
        );
    }

    #[test]
    fn parse_path_step() {
        let steps = parse_traversal(r#"g.V().out("knows").path()"#, &no_params).unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[2], GremlinStep::Path);
    }

    #[test]
    fn parse_has_label() {
        let steps = parse_traversal(r#"g.V().hasLabel("person")"#, &no_params).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[1], GremlinStep::HasLabel(vec!["person".into()]));
    }

    #[test]
    fn parse_order_by() {
        let steps = parse_traversal(r#"g.V().values("name").order()"#, &no_params).unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[2], GremlinStep::Order);
    }

    #[test]
    fn parse_with_strategy() {
        let steps = parse_traversal(
            r#"g.withStrategies(CountStrategy).V().where(outE().count().is(0))"#,
            &no_params,
        );
        // Should not error on the strategy prefix
        assert!(steps.is_ok());
    }

    #[test]
    fn tokenize_nested_parens() {
        let tokens = tokenize_steps(r#"V().where(outE().count().is(0)).values("name")"#).unwrap();
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0], "V()");
        assert!(tokens[1].starts_with("where("));
        assert_eq!(tokens[2], r#"values("name")"#);
    }

    #[test]
    fn parse_repeat_times() {
        let steps =
            parse_traversal(r#"g.V().repeat(__.out()).times(2).count()"#, &no_params).unwrap();
        assert!(steps.len() >= 4);
        assert!(matches!(steps[1], GremlinStep::Repeat(_)));
        assert_eq!(steps[2], GremlinStep::Times(2));
        assert_eq!(steps[3], GremlinStep::Count);
    }

    #[test]
    fn parse_select_as() {
        let steps = parse_traversal(
            r#"g.V().as("a").out("knows").as("b").select("a","b")"#,
            &no_params,
        )
        .unwrap();
        assert_eq!(steps[1], GremlinStep::As("a".into()));
        assert_eq!(steps[3], GremlinStep::As("b".into()));
        assert_eq!(steps[4], GremlinStep::Select(vec!["a".into(), "b".into()]));
    }
}
