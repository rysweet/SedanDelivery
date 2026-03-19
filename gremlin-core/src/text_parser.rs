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

    // Extract withSack initial value if present before stripping
    let sack_init = extract_with_sack_value(input, resolve_param);

    // Strip strategy wrappers: g.withStrategies(...).V()... or g.withoutStrategies(...).V()...
    // Also strip g.withSideEffect(...) and g.withSack(...) and g.withComputer(...)
    let owned;
    let input = if input.starts_with("g.withStrategies(")
        || input.starts_with("g.withoutStrategies(")
        || input.starts_with("g.withSideEffect(")
        || input.starts_with("g.withSack(")
        || input.starts_with("g.withComputer(")
        || input.starts_with("g.withBulk(")
        || input.starts_with("g.with(")
    {
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

    // Emit WithSack as first step if present
    if let Some(init_val) = sack_init {
        steps.push(GremlinStep::WithSack(init_val));
    }

    for token in &tokens {
        parse_step_token(token, resolve_param, &mut steps)?;
    }

    Ok(steps)
}

/// Strip `g.withStrategies(...)`, `g.withoutStrategies(...)`, `g.withSideEffect(...)`,
/// `g.withSack(...)`, `g.withComputer(...)`, or `g.with(...)` prefix.
/// Returns a new string starting with `g.` followed by the real traversal steps.
/// Handles chained `with*` prefixes (e.g., `g.withSideEffect(...).withSideEffect(...).V()`).
fn strip_strategy_prefix(input: &str) -> String {
    let mut result = input.to_string();
    loop {
        let after = if let Some(r) = result.strip_prefix("g.withStrategies(") {
            skip_balanced_parens(r)
        } else if let Some(r) = result.strip_prefix("g.withoutStrategies(") {
            skip_balanced_parens(r)
        } else if let Some(r) = result.strip_prefix("g.withSideEffect(") {
            skip_balanced_parens(r)
        } else if let Some(r) = result.strip_prefix("g.withSack(") {
            skip_balanced_parens(r)
        } else if let Some(r) = result.strip_prefix("g.withComputer(") {
            skip_balanced_parens(r)
        } else if let Some(r) = result.strip_prefix("g.withBulk(") {
            skip_balanced_parens(r)
        } else if let Some(r) = result.strip_prefix("g.with(") {
            skip_balanced_parens(r)
        } else {
            break;
        };

        let rest = after.strip_prefix('.').unwrap_or(after);
        result = format!("g.{rest}");
    }
    result
}

/// Extract the initial value from `g.withSack(value)` if present.
fn extract_with_sack_value(
    input: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Option<GremlinValue> {
    let prefix = "g.withSack(";
    if !input.starts_with(prefix) {
        if let Some(pos) = input.find(".withSack(") {
            let after = &input[pos + ".withSack(".len()..];
            return parse_sack_arg(after, resolve_param);
        }
        return None;
    }
    let after = &input[prefix.len()..];
    parse_sack_arg(after, resolve_param)
}

fn parse_sack_arg(
    after: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Option<GremlinValue> {
    let mut depth = 1;
    let mut end = 0;
    for (i, c) in after.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if end == 0 {
        return None;
    }
    let arg_str = after[..end].trim();
    // Only take first arg (before comma) — ignore split/merge operators
    let first_arg = arg_str.split(',').next().unwrap_or(arg_str).trim();
    parse_literal_value(first_arg, resolve_param)
}

fn parse_literal_value(
    s: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Option<GremlinValue> {
    if let Some(stripped) = s
        .strip_suffix('b')
        .or_else(|| s.strip_suffix('s'))
        .or_else(|| s.strip_suffix('i'))
        .or_else(|| s.strip_suffix('l'))
    {
        if let Ok(v) = stripped.parse::<i64>() {
            return Some(GremlinValue::Integer(v));
        }
    }
    if let Some(stripped) = s.strip_suffix('d').or_else(|| s.strip_suffix('f')) {
        if let Ok(v) = stripped.parse::<f64>() {
            return Some(GremlinValue::Float(v));
        }
    }
    if let Ok(v) = s.parse::<i64>() {
        return Some(GremlinValue::Integer(v));
    }
    if let Ok(v) = s.parse::<f64>() {
        return Some(GremlinValue::Float(v));
    }
    let unquoted = s.trim_matches('"').trim_matches('\'');
    if unquoted != s {
        return Some(GremlinValue::String(unquoted.to_string()));
    }
    resolve_param(s).or(None)
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
/// Trims whitespace from each token (multiline traversals have `\n` and leading spaces).
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
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    tokens.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        tokens.push(trimmed);
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
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::HasLabel(args));
        }
        "hasId" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            out.push(GremlinStep::HasId(args));
        }
        "hasNot" => {
            // hasNot("key") — filter elements where property does NOT exist
            let args = parse_string_arg_list(args_str)?;
            if let Some(key) = args.into_iter().next() {
                // Wrap in Not(Has(key, None)) to negate the existence check
                out.push(GremlinStep::Not(vec![GremlinStep::Has(key, None)]));
            }
        }
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
        "values" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Values(args));
        }
        "valueMap" => {
            let args = parse_string_arg_list(args_str)?;
            // valueMap(true, ...) or valueMap(true) includes tokens (id/label)
            let (with_tokens, keys) = if args.first().map(|s| s.as_str()) == Some("true") {
                (true, args[1..].to_vec())
            } else {
                (false, args)
            };
            out.push(GremlinStep::ValueMap(keys, with_tokens));
        }
        "properties" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Properties(args));
        }
        "id" => out.push(GremlinStep::Id),
        "label" => out.push(GremlinStep::Label),
        "select" => {
            let args = parse_string_arg_list(args_str)?;
            // Filter out Pop.* arguments (Pop.first, Pop.last, Pop.all, Pop.mixed)
            let filtered: Vec<String> = args
                .into_iter()
                .filter(|a| !a.starts_with("Pop."))
                .collect();
            out.push(GremlinStep::Select(filtered));
        }
        "as" => {
            let args = parse_string_arg_list(args_str)?;
            if let Some(alias) = args.into_iter().next() {
                out.push(GremlinStep::As(alias));
            }
        }
        "count" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let is_local = args
                .iter()
                .any(|a| matches!(a, GremlinValue::String(s) if s == "Scope.local"));
            if is_local {
                // count(local) — count elements in the list/map value
                out.push(GremlinStep::Length);
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
        "fold" => out.push(GremlinStep::Fold),
        "unfold" => out.push(GremlinStep::Unfold),
        "order" => out.push(GremlinStep::Order),
        "by" => parse_by_step(args_str, out)?,
        "limit" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            // Detect Scope.local
            let is_local = args
                .iter()
                .any(|a| matches!(a, GremlinValue::String(s) if s == "Scope.local"));
            // limit(n) or limit(Scope.global, n) — skip Scope arg
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
            // Detect Scope.local
            let is_local = args
                .iter()
                .any(|a| matches!(a, GremlinValue::String(s) if s == "Scope.local"));
            // Collect integer args
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
            } else if ints.len() == 1 {
                // Single int with scope — not meaningful, ignore
            }
        }
        "dedup" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Dedup(args));
        }
        "is" => {
            // Handle chained predicates: P.eq(1).and(P.gt(0)) or P.eq(1).or(P.lt(0))
            let s = args_str.trim();
            let chain_parts = split_predicate_chain(s);
            if chain_parts.len() > 1 {
                // Parse each part individually
                let mut parsed: Vec<(String, GremlinStep)> = Vec::new();
                for (op, part) in &chain_parts {
                    if let Some(pred) = try_parse_predicate_call(part, resolve_param) {
                        parsed.push((op.to_string(), GremlinStep::Is(pred)));
                    }
                }
                // Group into AND-chains separated by OR operators.
                // P.gt(18).and(P.lt(30)).or(P.gt(35)) → (gt(18) AND lt(30)) OR gt(35)
                let mut or_groups: Vec<Vec<GremlinStep>> = vec![vec![]];
                for (op, step) in parsed {
                    if op == "or" {
                        or_groups.push(vec![step]);
                    } else {
                        or_groups.last_mut().unwrap().push(step);
                    }
                }
                if or_groups.len() == 1 {
                    // All AND — emit as sequential Is steps
                    for step in or_groups.into_iter().flatten() {
                        out.push(step);
                    }
                } else {
                    // Build Or of And groups
                    let mut or_branches: Vec<Vec<GremlinStep>> = Vec::new();
                    for group in or_groups {
                        if group.len() == 1 {
                            or_branches.push(group);
                        } else {
                            // Multiple AND predicates become a single branch
                            or_branches.push(group);
                        }
                    }
                    out.push(GremlinStep::Or(or_branches));
                }
            } else {
                let pred = parse_predicate(args_str, resolve_param)?;
                out.push(GremlinStep::Is(pred));
            }
        }
        "where" => {
            // where(P.neq("a")) or where("a", P.eq("b")) or where(__.out("knows"))
            let raw_args = split_top_level_args(args_str);
            if raw_args.len() == 2 {
                // where("a", P.eq("b")) — label comparison
                let first = raw_args[0].trim();
                let second = raw_args[1].trim();
                let label = unquote(first);
                if let Some(pred) = try_parse_predicate_call(second, resolve_param) {
                    // Convert to: select("a").is(pred) effectively
                    // Emit as Where with a select + is pattern
                    out.push(GremlinStep::Where(vec![
                        GremlinStep::Select(vec![label]),
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
            let inner = parse_from_to_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::From(inner));
        }
        "to" => {
            let inner = parse_from_to_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::To(inner));
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
            // aggregate("x") or aggregate(local, "x") / aggregate(Scope.local, "x")
            let mut iter = args.into_iter();
            if let Some(first) = iter.next() {
                if first == "local"
                    || first == "Scope.local"
                    || first == "global"
                    || first == "Scope.global"
                {
                    // Scope argument — use the next arg as key
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
            let traversals = parse_multi_traversal_args(args_str, resolve_param)?;
            out.push(GremlinStep::Match(traversals));
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
                    // 1-arg choose: value-based dispatch with option() modifiers
                    out.push(GremlinStep::Choose(traversals[0].clone(), Vec::new(), None));
                }
                _ => {
                    out.push(GremlinStep::Choose(Vec::new(), Vec::new(), None));
                }
            }
        }
        "sideEffect" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::SideEffect(inner));
        }
        "identity" => out.push(GremlinStep::Identity),
        "tail" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            // Detect Scope.local
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
        "skip" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if let Some(GremlinValue::Integer(n)) = args.first() {
                out.push(GremlinStep::Skip(*n));
            }
        }
        "elementMap" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::ElementMap(args));
        }
        "hasKey" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::HasKey(args));
        }
        "hasValue" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            out.push(GremlinStep::HasValue(args));
        }
        "optional" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Optional(inner));
        }
        "simplePath" => out.push(GremlinStep::SimplePath),
        "cyclicPath" => out.push(GremlinStep::CyclicPath),
        "coin" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let p = match args.first() {
                Some(GremlinValue::Float(f)) => *f,
                Some(GremlinValue::Integer(i)) => *i as f64,
                _ => 0.5,
            };
            out.push(GremlinStep::Coin(p));
        }
        "map" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Map(inner));
        }
        "flatMap" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::FlatMap(inner));
        }
        "filter" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::FilterTraversal(inner));
        }
        "none" => {
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::NonePred(pred));
            } else {
                out.push(GremlinStep::None);
            }
        }
        "barrier" => out.push(GremlinStep::Barrier),
        "sample" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let n = if let Some(GremlinValue::Integer(v)) = args.first() {
                *v
            } else {
                1
            };
            out.push(GremlinStep::Sample(n));
        }
        "discard" => out.push(GremlinStep::Discard),
        "loops" => out.push(GremlinStep::Loops),
        "local" => {
            // local(traversal) — apply traversal to each element locally
            // Semantically similar to map(traversal)
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Map(inner));
        }
        "option" => {
            // option(key, traversal) — modifier for choose/branch
            let args = parse_arg_list(args_str, resolve_param)?;
            if args.len() >= 2 {
                // First arg: the option key (Pick.none maps to "none" string)
                let token = args[0].clone();
                // Remaining args contain the traversal steps.
                // Parse the original args_str to extract the traversal part.
                let inner_traversal = parse_option_traversal(args_str, resolve_param)?;
                out.push(GremlinStep::Option(token, inner_traversal));
            } else if args.len() == 1 {
                // option(Pick.none, discard) might parse as single arg
                let token = args[0].clone();
                out.push(GremlinStep::Option(token, vec![GremlinStep::Identity]));
            }
        }
        "value" => {
            // value() — extract the value from a property
            // Equivalent to values() with no keys on a property element
            out.push(GremlinStep::Values(vec![]));
        }
        "key" => {
            // key() — extract the key from a property
            out.push(GremlinStep::Label);
        }
        "store" => {
            // store("key") — like aggregate but uses lazy semantics
            let args = parse_string_arg_list(args_str)?;
            if let Some(key) = args.into_iter().next() {
                out.push(GremlinStep::Aggregate(key));
            }
        }
        "branch" => {
            // branch(traversal) — general branch step; choose is the more common form
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Choose(inner, Vec::new(), None));
        }
        "sack" => {
            let args_trimmed = args_str.trim();
            if args_trimmed.is_empty() {
                // sack() — returns the sack value
                out.push(GremlinStep::Sack);
            } else {
                // sack(operator) — apply binary op
                let op_name = args_trimmed.replace("Operator.", "").trim().to_lowercase();
                out.push(GremlinStep::SackOp(op_name));
            }
        }
        "with" | "withPath" | "withBulk" => {
            // with(WithOptions.tokens) — set tokens flag on preceding valueMap
            let args = parse_string_arg_list(args_str)?;
            if args.iter().any(|a| a == "WithOptions.tokens") {
                // Find the last ValueMap in the output and set its tokens flag
                for step in out.iter_mut().rev() {
                    if let GremlinStep::ValueMap(_, ref mut with_tokens) = step {
                        *with_tokens = true;
                        break;
                    }
                }
            }
            // withPath — enable path tracking
            // withBulk — enable bulk optimization
            // withBulk — enable bulk optimization
        }
        "element" => {
            // element() — return the element from a property
            out.push(GremlinStep::Identity);
        }
        "math" => {
            // math("expression") — mathematical expression on traversal values
            // Not fully implementable, but parse as identity to avoid blocking
            out.push(GremlinStep::Identity);
        }
        "index" => {
            // index() — adds index to each element
            out.push(GremlinStep::Identity);
        }
        // ── String steps ────────────────────────────────────────────────────
        "toLower" => {
            out.push(GremlinStep::ToLower);
        }
        "toUpper" => {
            out.push(GremlinStep::ToUpper);
        }
        "trim" => {
            out.push(GremlinStep::Trim);
        }
        "lTrim" => {
            out.push(GremlinStep::LTrim);
        }
        "rTrim" => {
            out.push(GremlinStep::RTrim);
        }
        "length" => {
            out.push(GremlinStep::Length);
        }
        "reverse" => {
            out.push(GremlinStep::Reverse);
        }
        "replace" => {
            let args = parse_string_arg_list(args_str)?;
            if args.len() >= 2 {
                out.push(GremlinStep::Replace(args[0].clone(), args[1].clone()));
            } else {
                return Err(TextParseError::InvalidArgument(
                    "replace requires 2 arguments".into(),
                ));
            }
        }
        "split" => {
            let args = parse_string_arg_list(args_str)?;
            if !args.is_empty() {
                out.push(GremlinStep::Split(args[0].clone()));
            } else {
                return Err(TextParseError::InvalidArgument(
                    "split requires 1 argument".into(),
                ));
            }
        }
        "substring" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            // Check if first arg is Scope.local
            let (offset, is_local) = if matches!(args.first(), Some(GremlinValue::String(s)) if s.contains("local"))
            {
                (1, true)
            } else {
                (0, false)
            };
            let start = match args.get(offset) {
                Some(GremlinValue::Integer(n)) => *n,
                _ => 0,
            };
            let len = match args.get(offset + 1) {
                Some(GremlinValue::Integer(n)) => Some(*n),
                _ => None,
            };
            if is_local {
                out.push(GremlinStep::SubstringLocal(start, len));
            } else {
                out.push(GremlinStep::Substring(start, len));
            }
        }
        "concat" => {
            // concat() args can be string literals, null, or traversals
            let raw_args = split_top_level_args(args_str);
            let mut traversals = Vec::new();
            for arg in &raw_args {
                let s = arg.trim();
                if s.is_empty() {
                    continue;
                }
                // String literal: "c"
                if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                    let inner = &s[1..s.len() - 1];
                    let unescaped = inner.replace("\\\"", "\"").replace("\\\\", "\\");
                    traversals.push(vec![GremlinStep::Constant(GremlinValue::String(unescaped))]);
                } else if s == "null" {
                    traversals.push(vec![GremlinStep::Constant(GremlinValue::Null)]);
                } else {
                    // Sub-traversal
                    let inner = parse_inner_traversal(s, resolve_param)?;
                    traversals.push(inner);
                }
            }
            out.push(GremlinStep::Concat(traversals));
        }
        "conjoin" => {
            let args = parse_string_arg_list(args_str)?;
            let sep = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::Conjoin(sep));
        }
        "format" => {
            let args = parse_string_arg_list(args_str)?;
            let pattern = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::Format(pattern));
        }
        "asString" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::AsStringLocal);
            } else {
                out.push(GremlinStep::AsString);
            }
        }
        "asNumber" => {
            out.push(GremlinStep::AsNumber);
        }
        "asBool" => {
            out.push(GremlinStep::AsBool);
        }
        "typeOf" => {
            out.push(GremlinStep::TypeOf);
        }
        // ── Collection steps ────────────────────────────────────────────────
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
            // all(P.gt(0)) — all elements match predicate
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::All(pred));
            } else {
                out.push(GremlinStep::All(Predicate::Neq(GremlinValue::Null)));
            }
        }
        "any" => {
            // any(P.gt(0)) — any element matches predicate
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::Any(pred));
            } else {
                out.push(GremlinStep::Any(Predicate::Neq(GremlinValue::Null)));
            }
        }
        // Steps still unsupported (provider-specific or not applicable)
        "profile" | "iterate" | "explain" | "tree" | "subgraph" | "asDate" | "dateAdd"
        | "dateDiff" | "fail" | "read" | "write" | "mergeV" | "mergeE" | "connectedComponent"
        | "shortestPath" | "pageRank" | "peerPressure" | "call" => {
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
            match key {
                GremlinValue::String(k) => {
                    out.push(GremlinStep::Has(k, None));
                }
                GremlinValue::Null => {
                    // has(null) — nothing has a null key, filter all
                    out.push(GremlinStep::Is(Predicate::Eq(GremlinValue::Null)));
                }
                _ => {}
            }
        }
        2 => {
            let key = parse_single_value(raw_args[0].trim(), resolve_param)?;
            let val_str = raw_args[1].trim();

            // has(null, ...) — always false
            if matches!(key, GremlinValue::Null) {
                out.push(GremlinStep::Is(Predicate::Eq(GremlinValue::Null)));
                return Ok(());
            }

            // has(T.label, X) → hasLabel(X)
            if let GremlinValue::String(ref k) = key {
                if k == "~label" {
                    if let Some(pred) = try_parse_predicate_call(val_str, resolve_param) {
                        // has(T.label, P.eq("person")) → hasLabel("person")
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
                            // has(T.label, null) — nothing matches null label
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

            // Check if second arg is a predicate (P.gt, P.within, TextP.*, etc.)
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
            // has(label, key, value) or has(label, key, P.*)
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

    // Handle single-arg by(Order.desc) or by(Order.asc) — sort current value
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

    // Handle T.label, T.id
    if first == "T.label" || first == "t[label]" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~label".to_string(), descending));
        return Ok(());
    }
    if first == "T.id" || first == "t[id]" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~id".to_string(), descending));
        return Ok(());
    }

    // Handle Column.keys, Column.values
    if first == "Column.keys" || first == "Column.values" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By(first.to_string(), descending));
        return Ok(());
    }

    let first_unquoted = unquote(first);

    // Check for Order.asc, Order.desc, Order.shuffle as second arg
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
/// Also handles bare predicates like `eq(30)`, `gt(30)`.
fn try_parse_predicate_call(
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
            let pred = try_parse_predicate_single(part, resolve_param)?;
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

fn try_parse_predicate_single(
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
            | "containing" | "notContaining" | "startingWith" | "notStartingWith"
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
fn split_predicate_chain(s: &str) -> Vec<(&str, &str)> {
    let mut parts = Vec::new();
    let mut depth = 0u32;
    let mut last_start = 0;
    let mut last_op = "first";
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && i + 1 < bytes.len() {
                    // Check if followed by .and( or .or(
                    let rest = &s[i + 1..];
                    if rest.starts_with(".and(") {
                        let part = &s[last_start..=i];
                        // Non-first parts have an extra trailing ) from the
                        // .and()/.or() wrapper — strip it.
                        let part = if last_op != "first" {
                            part.strip_suffix(')').unwrap_or(part)
                        } else {
                            part
                        };
                        parts.push((last_op, part));
                        last_op = "and";
                        // skip ".and(" — 5 chars from rest start (i+1), content at i+6
                        last_start = i + 6;
                        i += 6;
                        depth = 1;
                        continue;
                    } else if rest.starts_with(".or(") {
                        let part = &s[last_start..=i];
                        let part = if last_op != "first" {
                            part.strip_suffix(')').unwrap_or(part)
                        } else {
                            part
                        };
                        parts.push((last_op, part));
                        last_op = "or";
                        // skip ".or(" — 4 chars from rest start (i+1), content at i+5
                        last_start = i + 5;
                        i += 5;
                        depth = 1;
                        continue;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    if last_start < s.len() {
        let end = s.len();
        // Remove trailing ) that was part of the .and()/.or() wrapper
        let part = &s[last_start..end];
        let part = part.strip_suffix(')').unwrap_or(part);
        parts.push((last_op, part));
    }
    // If only one part (no chaining), return empty to signal no chain
    if parts.len() <= 1 && last_op == "first" {
        return vec![];
    }
    parts
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

/// Parse a collection step argument. Handles both literal values (null, 2, [1,2,3])
/// and sub-traversals (__.V().values("age").fold()).
fn parse_collection_step_arg(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Vec<GremlinStep>, TextParseError> {
    let s = args_str.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    // Detect literal null
    if s == "null" {
        return Ok(vec![GremlinStep::Constant(GremlinValue::Null)]);
    }
    // Detect list literal [...]
    if s.starts_with('[') && s.ends_with(']') {
        if let Ok(vals) = parse_arg_list(&s[1..s.len() - 1], resolve_param) {
            return Ok(vec![GremlinStep::Inject(vec![GremlinValue::List(vals)])]);
        }
    }
    // Detect simple numeric/boolean literals (not a traversal)
    if let Ok(n) = s.parse::<i64>() {
        return Ok(vec![GremlinStep::Constant(GremlinValue::Integer(n))]);
    }
    if let Ok(f) = s.parse::<f64>() {
        return Ok(vec![GremlinStep::Constant(GremlinValue::Float(f))]);
    }
    if s == "true" {
        return Ok(vec![GremlinStep::Constant(GremlinValue::Bool(true))]);
    }
    if s == "false" {
        return Ok(vec![GremlinStep::Constant(GremlinValue::Bool(false))]);
    }
    // Detect quoted string literal
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        let unescaped = inner.replace("\\\"", "\"").replace("\\\\", "\\");
        return Ok(vec![GremlinStep::Constant(GremlinValue::String(unescaped))]);
    }
    // Numeric with type suffix
    if s.len() > 1 {
        let suffix = s.as_bytes()[s.len() - 1];
        let num_part = &s[..s.len() - 1];
        if matches!(suffix, b'b' | b's' | b'i' | b'l' | b'L' | b'n') {
            if let Ok(n) = num_part.parse::<i64>() {
                return Ok(vec![GremlinStep::Constant(GremlinValue::Integer(n))]);
            }
        }
        if matches!(suffix, b'f' | b'd') {
            if let Ok(f) = num_part.parse::<f64>() {
                return Ok(vec![GremlinStep::Constant(GremlinValue::Float(f))]);
            }
        }
    }
    // Fall back to sub-traversal parsing
    parse_inner_traversal(args_str, resolve_param)
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

/// Parse `option(key, traversal)` — skip the first arg (key) and return the traversal.
fn parse_option_traversal(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Vec<GremlinStep>, TextParseError> {
    let raw_args = split_top_level_args(args_str);
    if raw_args.len() >= 2 {
        // The second argument onward is the traversal
        let traversal_str = raw_args[1..].join(",");
        parse_inner_traversal(traversal_str.trim(), resolve_param)
    } else {
        Ok(vec![GremlinStep::Identity])
    }
}

/// Parse a `from(...)` or `to(...)` argument — could be a string label or a traversal.
fn parse_from_to_traversal(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Vec<GremlinStep>, TextParseError> {
    let s = args_str.trim();
    // If it's a quoted string, it's a step label reference
    if s.starts_with('"') {
        let label = unquote(s);
        return Ok(vec![GremlinStep::Select(vec![label])]);
    }
    // Otherwise parse as a full traversal
    let inner = parse_inner_traversal(s, resolve_param)?;
    if inner.is_empty() {
        Ok(vec![GremlinStep::Select(vec![])])
    } else {
        Ok(inner)
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
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
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

/// Parse a list of string arguments, resolving parameter references.
fn parse_string_arg_list_with_params(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<Vec<String>, TextParseError> {
    if args_str.trim().is_empty() {
        return Ok(Vec::new());
    }
    let raw_args = split_top_level_args(args_str);
    let mut result = Vec::new();
    for arg in raw_args {
        let s = arg.trim();
        if let Some(GremlinValue::String(val)) = resolve_param(s) {
            result.push(val);
        } else {
            result.push(unquote(s));
        }
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

    // String literal (double quotes)
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        let unescaped = inner.replace("\\\"", "\"").replace("\\\\", "\\");
        return Ok(GremlinValue::String(unescaped));
    }

    // String literal (single quotes)
    if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        let unescaped = inner.replace("\\'", "'").replace("\\\\", "\\");
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

    // NaN / Infinity
    if s == "NaN" || s == "Double.NaN" || s == "Float.NaN" {
        return Ok(GremlinValue::Float(f64::NAN));
    }
    if s == "Infinity" || s == "+Infinity" || s == "Double.POSITIVE_INFINITY" {
        return Ok(GremlinValue::Float(f64::INFINITY));
    }
    if s == "-Infinity" || s == "Double.NEGATIVE_INFINITY" {
        return Ok(GremlinValue::Float(f64::NEG_INFINITY));
    }

    // List literal: [1, 2, "a", null]
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        let items = parse_arg_list(inner, resolve_param)?;
        return Ok(GremlinValue::List(items));
    }

    // Integer
    if let Ok(n) = s.parse::<i64>() {
        return Ok(GremlinValue::Integer(n));
    }

    // Float
    if let Ok(f) = s.parse::<f64>() {
        return Ok(GremlinValue::Float(f));
    }

    // Numeric with type suffix: 1b (byte), 2s (short), 3i (int), 4L (long), 0.5f/0.5d (float/double)
    if s.len() > 1 {
        let suffix = s.as_bytes()[s.len() - 1];
        let num_part = &s[..s.len() - 1];
        if matches!(
            suffix,
            b'b' | b's' | b'i' | b'l' | b'L' | b'n' | b'B' | b'S' | b'I' | b'N'
        ) {
            if let Ok(n) = num_part.parse::<i64>() {
                return Ok(GremlinValue::Integer(n));
            }
        }
        if matches!(suffix, b'f' | b'd' | b'F' | b'D' | b'm' | b'M') {
            if let Ok(f) = num_part.parse::<f64>() {
                return Ok(GremlinValue::Float(f));
            }
        }
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

    // Pick.none — default branch marker for choose/option
    if s == "Pick.none" {
        return Ok(GremlinValue::String("none".to_string()));
    }

    // GType.* — Gremlin type constants
    if let Some(gtype) = s.strip_prefix("GType.") {
        return Ok(GremlinValue::String(format!("GType.{gtype}")));
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

/// Normalize a GType name to the Gremlin type string used by typeOf().
fn normalize_gtype(s: &str) -> String {
    match s.to_uppercase().as_str() {
        "STRING" | "JAVA_LANG_STRING" => "string".to_string(),
        "INT" | "INTEGER" | "JAVA_LANG_INTEGER" => "integer".to_string(),
        "LONG" | "JAVA_LANG_LONG" => "long".to_string(),
        "DOUBLE" | "JAVA_LANG_DOUBLE" => "double".to_string(),
        "FLOAT" | "JAVA_LANG_FLOAT" => "float".to_string(),
        "BOOLEAN" | "JAVA_LANG_BOOLEAN" => "boolean".to_string(),
        "MAP" | "JAVA_UTIL_MAP" => "map".to_string(),
        "LIST" | "JAVA_UTIL_LIST" => "list".to_string(),
        "SET" | "JAVA_UTIL_SET" => "set".to_string(),
        "SHORT" | "JAVA_LANG_SHORT" => "short".to_string(),
        "BYTE" | "JAVA_LANG_BYTE" => "byte".to_string(),
        "BIGINTEGER" | "JAVA_MATH_BIGINTEGER" => "biginteger".to_string(),
        "BIGDECIMAL" | "JAVA_MATH_BIGDECIMAL" => "bigdecimal".to_string(),
        other => other.to_lowercase(),
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
