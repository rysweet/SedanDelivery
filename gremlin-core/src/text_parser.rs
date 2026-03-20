//! Simple Gremlin text parser for TinkerPop TCK `.feature` file traversals.
//!
//! Parses Gremlin traversal strings like `g.V().has("name","marko").out("knows")`
//! into [`GremlinStep`] sequences.  This is intentionally not a full Gremlin
//! grammar parser — it covers the step patterns that appear in the downloaded
//! TinkerPop TCK feature files.

use crate::bytecode::{GremlinStep, Predicate};
use crate::types::GremlinValue;
use indexmap::IndexMap;

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

    // Extract withSideEffect values before stripping
    let side_effects = extract_with_side_effects(input, resolve_param);

    // Detect strategy flags before stripping
    let productive_by = input.contains("ProductiveByStrategy")
        && !input.contains("withoutStrategies(ProductiveByStrategy");
    // ReadOnlyStrategy: only active with withStrategies (not withoutStrategies)
    let read_only = input.contains("withStrategies(")
        && input.contains("ReadOnlyStrategy")
        && !input.contains("withoutStrategies(ReadOnlyStrategy");
    // EdgeLabelVerificationStrategy(throwException: true)
    let edge_label_verify = input.contains("withStrategies(")
        && input.contains("EdgeLabelVerificationStrategy")
        && input.contains("throwException: true")
        && !input.contains("withoutStrategies(EdgeLabelVerificationStrategy");
    // ReservedKeysVerificationStrategy(throwException: true, keys: {...})
    let reserved_keys = extract_reserved_keys_strategy(input);
    // SeedStrategy(seed: 999999) — extract seed value for deterministic random
    let seed_strategy = extract_seed_strategy(input);

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

    // Emit WithSideEffect as first steps
    for (key, val, op) in side_effects {
        steps.push(GremlinStep::WithSideEffect(key, val, op));
    }

    // Emit WithSack as first step if present
    if let Some(init_val) = sack_init {
        steps.push(GremlinStep::WithSack(init_val));
    }

    // Emit strategy flags
    if productive_by {
        steps.push(GremlinStep::ProductiveByStrategy);
    }
    if read_only {
        steps.push(GremlinStep::ReadOnlyStrategy);
    }
    if let Some(keys) = reserved_keys {
        steps.push(GremlinStep::ReservedKeysVerificationStrategy(keys));
    }
    if edge_label_verify {
        steps.push(GremlinStep::EdgeLabelVerificationStrategy);
    }
    if let Some(seed) = seed_strategy {
        steps.push(GremlinStep::SeedStrategy(seed));
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

/// Extract reserved keys from `ReservedKeysVerificationStrategy(throwException: true, keys: {"age"})`.
/// Returns `Some(extra_keys)` if the strategy is active with throwException: true,
/// where `extra_keys` are user-specified reserved keys beyond the defaults ("id", "label").
/// Extract seed value from `SeedStrategy(seed: 999999)` if present.
fn extract_seed_strategy(input: &str) -> Option<i64> {
    if !input.contains("SeedStrategy") || input.contains("withoutStrategies(") {
        return None;
    }
    let start = input.find("SeedStrategy(")?;
    let after = &input[start + "SeedStrategy(".len()..];
    // Find "seed:" or "seed :"
    let seed_pos = after.find("seed")?;
    let rest = &after[seed_pos + 4..];
    let rest = rest.trim_start().strip_prefix(':')?;
    let rest = rest.trim_start();
    // Parse the integer value
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(rest.len());
    rest[..end].parse::<i64>().ok()
}

fn extract_reserved_keys_strategy(input: &str) -> Option<Vec<String>> {
    if !input.contains("withStrategies(") || !input.contains("ReservedKeysVerificationStrategy") {
        return None;
    }
    if input.contains("withoutStrategies(ReservedKeysVerificationStrategy") {
        return None;
    }
    // Must have throwException: true
    let start = input.find("ReservedKeysVerificationStrategy(")?;
    let after = &input[start..];
    if !after.contains("throwException: true") {
        return None;
    }
    // Extract keys: {"age", "foo"} if present
    let mut extra_keys = Vec::new();
    if let Some(keys_pos) = after.find("keys:") {
        let rest = &after[keys_pos + 5..];
        if let Some(open) = rest.find('{') {
            if let Some(close) = rest[open..].find('}') {
                let inner = &rest[open + 1..open + close];
                for part in inner.split(',') {
                    let key = part.trim().trim_matches('"').trim_matches('\'');
                    if !key.is_empty() {
                        extra_keys.push(key.to_string());
                    }
                }
            }
        }
    }
    Some(extra_keys)
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
    // Integer with type suffix: 0b (byte), 0s (short), 0i (int), 0L/0l (long), 0n (BigInteger)
    if let Some(stripped) = s
        .strip_suffix('b')
        .or_else(|| s.strip_suffix('B'))
        .or_else(|| s.strip_suffix('s'))
        .or_else(|| s.strip_suffix('S'))
        .or_else(|| s.strip_suffix('i'))
        .or_else(|| s.strip_suffix('I'))
        .or_else(|| s.strip_suffix('l'))
        .or_else(|| s.strip_suffix('L'))
        .or_else(|| s.strip_suffix('n'))
        .or_else(|| s.strip_suffix('N'))
    {
        if let Ok(v) = stripped.parse::<i64>() {
            return Some(GremlinValue::Integer(v));
        }
        // BigInteger values too large for i64: parse as f64 for approximate arithmetic
        if let Ok(v) = stripped.parse::<f64>() {
            return Some(GremlinValue::Float(v));
        }
    }
    if let Some(stripped) = s
        .strip_suffix('d')
        .or_else(|| s.strip_suffix('D'))
        .or_else(|| s.strip_suffix('f'))
        .or_else(|| s.strip_suffix('F'))
        .or_else(|| s.strip_suffix('m'))
        .or_else(|| s.strip_suffix('M'))
    {
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
    // Boolean literals
    if s == "true" {
        return Some(GremlinValue::Bool(true));
    }
    if s == "false" {
        return Some(GremlinValue::Bool(false));
    }
    resolve_param(s).or(None)
}

/// Extract all `withSideEffect(key, value[, operator])` tuples from the input.
fn extract_with_side_effects(
    input: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Vec<(String, GremlinValue, Option<String>)> {
    let mut results = Vec::new();
    let mut search = input;
    while let Some(prefix) = search.find("withSideEffect(") {
        let after = &search[prefix + "withSideEffect(".len()..];
        // Find the matching close paren
        let mut depth = 1u32;
        let mut in_str = false;
        let mut escape = false;
        let mut end = 0;
        for (i, c) in after.char_indices() {
            if escape {
                escape = false;
                continue;
            }
            if c == '\\' && in_str {
                escape = true;
                continue;
            }
            if c == '"' {
                in_str = !in_str;
                continue;
            }
            if in_str {
                continue;
            }
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
            break;
        }
        let arg_str = &after[..end];
        // Split into key and value (first comma at top level)
        let mut comma_depth = 0;
        let mut comma_pos = None;
        let mut in_s = false;
        let mut esc = false;
        for (i, c) in arg_str.char_indices() {
            if esc {
                esc = false;
                continue;
            }
            if c == '\\' && in_s {
                esc = true;
                continue;
            }
            if c == '"' {
                in_s = !in_s;
                continue;
            }
            if in_s {
                continue;
            }
            match c {
                '(' | '[' | '{' => comma_depth += 1,
                ')' | ']' | '}' => comma_depth -= 1,
                ',' if comma_depth == 0 => {
                    if comma_pos.is_none() {
                        comma_pos = Some(i);
                    }
                    // Don't break — we want the first comma only for key/value split
                    break;
                }
                _ => {}
            }
        }
        if let Some(cp) = comma_pos {
            let key_str = arg_str[..cp].trim().trim_matches('"');
            let rest_str = arg_str[cp + 1..].trim();
            // Check for 3-arg form: value, Operator.xxx
            // Split rest_str at the last top-level comma to separate value from operator
            let mut last_comma = None;
            let mut ld = 0i32;
            let mut in_s2 = false;
            let mut esc2 = false;
            for (i2, c) in rest_str.char_indices() {
                if esc2 {
                    esc2 = false;
                    continue;
                }
                if c == '\\' && in_s2 {
                    esc2 = true;
                    continue;
                }
                if c == '"' {
                    in_s2 = !in_s2;
                    continue;
                }
                if in_s2 {
                    continue;
                }
                match c {
                    '(' | '[' | '{' => ld += 1,
                    ')' | ']' | '}' => ld -= 1,
                    ',' if ld == 0 => {
                        last_comma = Some(i2);
                    }
                    _ => {}
                }
            }
            let (val_str, operator) = if let Some(lc) = last_comma {
                let maybe_op = rest_str[lc + 1..].trim();
                if maybe_op.starts_with("Operator.") {
                    let op_name = maybe_op.strip_prefix("Operator.").unwrap_or("sum");
                    // Only split for operators that we actually reduce
                    // "assign" just means "collect normally" — skip it
                    if op_name == "assign" {
                        (rest_str, None)
                    } else {
                        (&rest_str[..lc], Some(op_name.to_string()))
                    }
                } else {
                    (rest_str, None)
                }
            } else {
                (rest_str, None)
            };
            // Try to parse the value — handle sets like {"alice"}, lists like ["x","y"], etc.
            if let Some(val) = parse_side_effect_value(val_str.trim(), resolve_param) {
                results.push((key_str.to_string(), val, operator));
            }
        }
        search = &after[end..];
    }
    results
}

/// Parse a withSideEffect value: strings, numbers, sets {}, lists [], etc.
fn parse_side_effect_value(
    s: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Option<GremlinValue> {
    let s = s.trim();
    // Handle Set notation: {"alice"} → List (treated as set)
    if s.starts_with('{') && s.ends_with('}') {
        let inner = &s[1..s.len() - 1];
        let items: Vec<GremlinValue> = inner
            .split(',')
            .filter_map(|item| {
                let item = item.trim().trim_matches('"');
                if item.is_empty() {
                    None
                } else {
                    Some(GremlinValue::String(item.to_string()))
                }
            })
            .collect();
        return Some(GremlinValue::List(items));
    }
    // Handle List notation: ["x","y"]
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        let items: Vec<GremlinValue> = inner
            .split(',')
            .filter_map(|item| parse_literal_value(item.trim(), resolve_param))
            .collect();
        return Some(GremlinValue::List(items));
    }
    // Handle "Set" keyword: skip the operator arg
    if s == "Set" || s.starts_with("Operator.") {
        return None;
    }
    parse_literal_value(s, resolve_param)
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
            // Try predicate form first: hasId(P.neq(xx1)), hasId(P.without([]))
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::HasIdPredicate(pred));
            } else {
                let args = parse_arg_list(args_str, resolve_param)?;
                out.push(GremlinStep::HasId(args));
            }
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
            let (token_mode, keys) = if args.first().map(|s| s.as_str()) == Some("true") {
                (1u8, args[1..].to_vec())
            } else {
                (0u8, args)
            };
            out.push(GremlinStep::ValueMap(keys, token_mode));
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
            for alias in args {
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
                // fold(seed, Operator.op) — reduce with binary operator
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
        "order" => {
            // Detect Scope.local
            if args_str.contains("Scope.local") {
                out.push(GremlinStep::SortLocal);
            } else {
                out.push(GremlinStep::Order);
            }
        }
        "by" => parse_by_step(args_str, out, resolve_param)?,
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
            // Detect Scope.local
            if args_str.contains("Scope.local") || args_str.contains("local") {
                // dedup(Scope.local) or dedup(Scope.local, "x", "y")
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
                // Check for property(Cardinality.xxx, Map) pattern — expand Map
                // into individual property steps.
                let is_cardinality = matches!(&args[0],
                    GremlinValue::String(s) if s.starts_with("Cardinality.") || s == "single" || s == "list" || s == "set"
                );
                if is_cardinality && matches!(&args[1], GremlinValue::Map(_)) {
                    if let GremlinValue::Map(map) = &args[1] {
                        for (k, v) in map {
                            out.push(GremlinStep::Property(k.clone(), v.clone()));
                        }
                    }
                } else if let GremlinValue::String(key) = &args[0] {
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
        "emit" => {
            let trimmed = args_str.trim();
            if trimmed.is_empty() {
                out.push(GremlinStep::Emit);
            } else {
                let inner = parse_inner_traversal(trimmed, resolve_param)?;
                if inner.is_empty() {
                    out.push(GremlinStep::Emit);
                } else {
                    out.push(GremlinStep::EmitPred(inner));
                }
            }
        }
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
            let is_local = args
                .iter()
                .any(|a| matches!(a, GremlinValue::String(s) if s == "Scope.local"));
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
        "elementMap" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::ElementMap(args));
        }
        "hasKey" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::HasKey(args));
        }
        "hasValue" => {
            // Try predicate first: hasValue(P.gt(30))
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::HasValuePredicate(pred));
            } else {
                let args = parse_arg_list(args_str, resolve_param)?;
                out.push(GremlinStep::HasValue(args));
            }
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
            let is_local = args
                .iter()
                .any(|a| matches!(a, GremlinValue::String(s) if s == "Scope.local"));
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
            // with(WithOptions.tokens[, WithOptions.labels|ids]) — set token mode
            let args = parse_string_arg_list(args_str)?;
            if args.iter().any(|a| a == "WithOptions.tokens") {
                let mode = if args.iter().any(|a| a == "WithOptions.labels") {
                    2u8 // labels only
                } else if args.iter().any(|a| a == "WithOptions.ids") {
                    3u8 // ids only
                } else {
                    1u8 // all tokens
                };
                for step in out.iter_mut().rev() {
                    if let GremlinStep::ValueMap(_, ref mut token_mode) = step {
                        *token_mode = mode;
                        break;
                    }
                }
            }
            // with(WithOptions.indexer, WithOptions.map) — convert index to map mode
            if args.iter().any(|a| a == "WithOptions.indexer")
                && args.iter().any(|a| a == "WithOptions.map")
            {
                for step in out.iter_mut().rev() {
                    if matches!(step, GremlinStep::Index) {
                        *step = GremlinStep::IndexWithMap;
                        break;
                    }
                }
            }
            // withPath — enable path tracking
            // withBulk — enable bulk optimization
        }
        "element" => {
            // element() — return the owning element from a property
            out.push(GremlinStep::Element);
        }
        "math" => {
            // math("expression") — mathematical expression on traversal values
            let args = parse_string_arg_list(args_str)?;
            let expr_str = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::Math(expr_str));
        }
        "index" => {
            // index() — adds index to each element
            out.push(GremlinStep::Index);
        }
        // ── String steps ────────────────────────────────────────────────────
        "toLower" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_tolower_local".into()));
            } else {
                out.push(GremlinStep::ToLower);
            }
        }
        "toUpper" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_toupper_local".into()));
            } else {
                out.push(GremlinStep::ToUpper);
            }
        }
        "trim" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_trim_local".into()));
            } else {
                out.push(GremlinStep::Trim);
            }
        }
        "lTrim" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_ltrim_local".into()));
            } else {
                out.push(GremlinStep::LTrim);
            }
        }
        "rTrim" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_rtrim_local".into()));
            } else {
                out.push(GremlinStep::RTrim);
            }
        }
        "length" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_length_local".into()));
            } else {
                out.push(GremlinStep::Length);
            }
        }
        "reverse" => {
            out.push(GremlinStep::Reverse);
        }
        "replace" => {
            let args = parse_string_arg_list(args_str)?;
            let is_local = args
                .first()
                .map(|a| a.contains("local") || a == "Scope.local")
                .unwrap_or(false);
            if is_local {
                // replace(local, old, new)
                if args.len() >= 3 {
                    out.push(GremlinStep::StringOpLocal(format!(
                        "gremlin_replace_local\x00{}\x00{}",
                        args[1], args[2]
                    )));
                }
            } else if args.len() >= 2 {
                out.push(GremlinStep::Replace(args[0].clone(), args[1].clone()));
            } else {
                return Err(TextParseError::InvalidArgument(
                    "replace requires 2 arguments".into(),
                ));
            }
        }
        "split" => {
            let args = parse_string_arg_list(args_str)?;
            let is_local = args
                .first()
                .map(|a| a.contains("local") || a == "Scope.local")
                .unwrap_or(false);
            if is_local {
                // split(local, sep)
                if args.len() >= 2 {
                    out.push(GremlinStep::StringOpLocal(format!(
                        "gremlin_split_local\x00{}",
                        args[1]
                    )));
                }
            } else if !args.is_empty() {
                out.push(GremlinStep::Split(args[0].clone()));
            } else {
                // split("") — split each character
                out.push(GremlinStep::Split(String::new()));
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
            let trimmed = args_str.trim();
            if trimmed.is_empty() {
                out.push(GremlinStep::AsNumber);
            } else if trimmed.contains("INT") || trimmed.contains("LONG") {
                out.push(GremlinStep::AsNumberTyped("int".into()));
            } else if trimmed.contains("FLOAT")
                || trimmed.contains("DOUBLE")
                || trimmed.contains("BIG_DECIMAL")
            {
                out.push(GremlinStep::AsNumberTyped("float".into()));
            } else if trimmed.contains("BYTE") {
                out.push(GremlinStep::AsNumberTyped("byte".into()));
            } else if trimmed.contains("SHORT") {
                out.push(GremlinStep::AsNumberTyped("short".into()));
            } else if trimmed.contains("VERTEX")
                || trimmed.contains("EDGE")
                || trimmed.contains("PROPERTY")
                || trimmed.contains("PATH")
            {
                out.push(GremlinStep::AsNumberTyped("__invalid__".into()));
            } else {
                out.push(GremlinStep::AsNumber);
            }
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
fn parse_by_step(
    args_str: &str,
    out: &mut Vec<GremlinStep>,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<(), TextParseError> {
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

    // Handle T.key — for ordering by property key name
    if first == "T.key" || first == "t[key]" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~key".to_string(), descending));
        return Ok(());
    }
    // Handle T.value — for ordering by property value
    if first == "T.value" || first == "t[value]" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~value".to_string(), descending));
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

    // Handle bare token references: by(label) = by(T.label), by(id) = by(T.id)
    if first == "label" || first == "Label" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~label".to_string(), descending));
        return Ok(());
    }
    if first == "id" || first == "Id" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~id".to_string(), descending));
        return Ok(());
    }
    if first == "key" || first == "Key" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~key".to_string(), descending));
        return Ok(());
    }
    if first == "value" || first == "Value" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~value".to_string(), descending));
        return Ok(());
    }

    // Check for Order.asc, Order.desc, Order.shuffle as second arg
    let descending = if raw_args.len() >= 2 {
        let second = raw_args[1].trim();
        second == "Order.desc" || second == "desc"
    } else {
        false
    };

    // Detect sub-traversal arguments: if the first arg looks like a traversal
    // (contains function calls like values(...), out(), label(), etc.)
    // parse it as a ByTraversal.
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

/// Check if a string looks like a Gremlin traversal rather than a property name.
/// Matches single-step calls like `values("name")`, `count()` and also chained
/// calls like `bothE().count()`, `out().values("name")`.
fn looks_like_traversal(s: &str) -> bool {
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
fn find_matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut string_char = '"';
    for (i, ch) in s.char_indices() {
        if in_string {
            if ch == string_char {
                in_string = false;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_string = true;
            string_char = ch;
        } else if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
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
pub(crate) fn parse_inner_traversal(
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

    // T.label / T.id as standalone traversal tokens (e.g., choose(T.label))
    if s == "T.label" || s == "t[label]" {
        return Ok(vec![GremlinStep::Label]);
    }
    if s == "T.id" || s == "t[id]" {
        return Ok(vec![GremlinStep::Id]);
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
    // Handle to(Direction.OUT, "label") / to(Direction.IN, "label") / to(Direction.BOTH, "label")
    // These are equivalent to out("label") / in("label") / both("label")
    let args = split_top_level_args(s);
    if !args.is_empty() {
        let first = args[0].trim();
        if first.starts_with("Direction.") || first == "OUT" || first == "IN" || first == "BOTH" {
            let labels: Vec<String> = args[1..].iter().map(|a| unquote(a.trim())).collect();
            return match first {
                "Direction.OUT" | "OUT" => Ok(vec![GremlinStep::Out(labels)]),
                "Direction.IN" | "IN" => Ok(vec![GremlinStep::In(labels)]),
                "Direction.BOTH" | "BOTH" => Ok(vec![GremlinStep::Both(labels)]),
                _ => Ok(vec![GremlinStep::Both(labels)]),
            };
        }
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
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
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

    // Set literal: {1, 2, 3} — deduplicated list
    if s.starts_with('{') && s.ends_with('}') {
        let inner = &s[1..s.len() - 1];
        let items = parse_arg_list(inner, resolve_param)?;
        let mut deduped: Vec<GremlinValue> = Vec::new();
        for item in items {
            if !deduped.contains(&item) {
                deduped.push(item);
            }
        }
        return Ok(GremlinValue::List(deduped));
    }

    // List or Map literal: [1, 2, "a"] or ["key": "val", ...]
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        // Detect Groovy map literal: ["key": "val", ...] — check for top-level ':'
        let mut depth = 0u32;
        let mut in_str = false;
        let mut escape = false;
        let mut has_colon = false;
        for ch in inner.chars() {
            if escape {
                escape = false;
                continue;
            }
            if ch == '\\' && in_str {
                escape = true;
                continue;
            }
            if ch == '"' || ch == '\'' {
                in_str = !in_str;
                continue;
            }
            if in_str {
                continue;
            }
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                ':' if depth == 0 => {
                    has_colon = true;
                    break;
                }
                _ => {}
            }
        }
        if has_colon {
            // Empty map: [:]
            if inner.trim() == ":" {
                return Ok(GremlinValue::Map(IndexMap::new()));
            }
            // Parse as map literal
            let parts = split_top_level_args(inner);
            let mut map = IndexMap::new();
            for part in &parts {
                let part = part.trim();
                // Find the first top-level ':'
                let mut d = 0u32;
                let mut in_s = false;
                let mut esc = false;
                let mut colon_pos = None;
                for (i, ch) in part.char_indices() {
                    if esc {
                        esc = false;
                        continue;
                    }
                    if ch == '\\' && in_s {
                        esc = true;
                        continue;
                    }
                    if ch == '"' || ch == '\'' {
                        in_s = !in_s;
                        continue;
                    }
                    if in_s {
                        continue;
                    }
                    match ch {
                        '(' | '[' | '{' => d += 1,
                        ')' | ']' | '}' => d = d.saturating_sub(1),
                        ':' if d == 0 => {
                            colon_pos = Some(i);
                            break;
                        }
                        _ => {}
                    }
                }
                if let Some(cp) = colon_pos {
                    let key_str = part[..cp].trim();
                    let val_str = part[cp + 1..].trim();
                    let key = unquote(key_str);
                    let val = parse_single_value(val_str, resolve_param)?;
                    map.insert(key, val);
                }
            }
            return Ok(GremlinValue::Map(map));
        }
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
