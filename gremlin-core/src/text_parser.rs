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
    // Also strip g.withSideEffect(...) and g.withSack(...) and g.withComputer(...)
    let owned;
    let input = if input.starts_with("g.withStrategies(")
        || input.starts_with("g.withoutStrategies(")
        || input.starts_with("g.withSideEffect(")
        || input.starts_with("g.withSack(")
        || input.starts_with("g.withComputer(")
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
            let args = parse_string_arg_list(args_str)?;
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
            // count() or count(Scope.local) — ignore scope argument
            out.push(GremlinStep::Count);
        }
        "sum" => {
            out.push(GremlinStep::Sum);
        }
        "min" => {
            out.push(GremlinStep::Min);
        }
        "max" => {
            out.push(GremlinStep::Max);
        }
        "mean" => {
            out.push(GremlinStep::Mean);
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
            let is_local = args.iter().any(|a| matches!(a, GremlinValue::String(s) if s == "Scope.local"));
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
            let is_local = args.iter().any(|a| matches!(a, GremlinValue::String(s) if s == "Scope.local"));
            // Collect integer args
            let ints: Vec<i64> = args.iter().filter_map(|a| {
                if let GremlinValue::Integer(v) = a { Some(*v) } else { None }
            }).collect();
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
            let pred = parse_predicate(args_str, resolve_param)?;
            out.push(GremlinStep::Is(pred));
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
            // aggregate("x") or aggregate(local, "x") / aggregate(Scope.local, "x")
            let mut iter = args.into_iter();
            if let Some(first) = iter.next() {
                if first == "local" || first == "Scope.local"
                    || first == "global" || first == "Scope.global"
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
            // tail(), tail(n), or tail(Scope.global, n) — skip Scope arg
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
            out.push(GremlinStep::Tail(n));
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
        "none" => out.push(GremlinStep::None),
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
            // sack() — returns the sack value or sets it; approximated as identity
            out.push(GremlinStep::Identity);
        }
        "with" | "withPath" | "withBulk" => {
            // with(key, value) — configuration modifier, typically no-op for query execution
            // withPath — enable path tracking
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
        // Steps still unsupported
        "profile" | "iterate" | "explain" | "tree" | "subgraph" | "concat" | "conjoin"
        | "toLower" | "toUpper" | "trim" | "lTrim" | "rTrim" | "length" | "replace" | "split"
        | "substring" | "reverse" | "format" | "asString" | "asBool" | "asNumber" | "asDate"
        | "dateAdd" | "dateDiff" | "fail" | "read" | "write" | "combine" | "intersect"
        | "difference" | "disjunct" | "merge" | "mergeV" | "mergeE" | "product" | "all" | "any"
        | "typeOf" | "connectedComponent" | "shortestPath" | "pageRank" | "peerPressure"
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
