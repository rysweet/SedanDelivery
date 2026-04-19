//! Strategy and subgraph filter parsing helpers.

use super::tokenizer::*;
use super::*;

/// Strip `g.withStrategies(...)`, `g.withoutStrategies(...)`, `g.withSideEffect(...)`,
/// `g.withSack(...)`, `g.withComputer(...)`, or `g.with(...)` prefix.
/// Returns a new string starting with `g.` followed by the real traversal steps.
/// Handles chained `with*` prefixes (e.g., `g.withSideEffect(...).withSideEffect(...).V()`).
pub(crate) fn strip_strategy_prefix(input: &str) -> String {
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
pub(crate) fn extract_seed_strategy(input: &str) -> Option<i64> {
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

pub(crate) fn extract_reserved_keys_strategy(input: &str) -> Option<Vec<String>> {
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

/// Extract the vertex filter traversal from SubgraphStrategy.
/// e.g., `SubgraphStrategy(vertices: __.has("name", P.within("josh", "lop", "ripple")))`
/// Returns the parsed filter steps if found.
pub(crate) fn extract_subgraph_vertex_filter(
    input: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Option<Vec<GremlinStep>> {
    if !input.contains("SubgraphStrategy") {
        return None;
    }
    let start = input.find("SubgraphStrategy(")?;
    let after = &input[start + "SubgraphStrategy(".len()..];

    // Find "vertices:" parameter
    let vpos = after.find("vertices:")?;
    let rest = after[vpos + "vertices:".len()..].trim_start();

    // Extract the traversal text up to the matching close or next parameter
    let traversal_text = extract_balanced_traversal(rest)?;

    // Parse the filter traversal
    parse_inner_traversal(&traversal_text, resolve_param).ok()
}

/// Extract a balanced traversal expression from a position in a string.
/// Handles nested parentheses and stops at `,` or `)` at depth 0.
pub(crate) fn extract_balanced_traversal(input: &str) -> Option<String> {
    let input = input.trim_start();
    let mut depth = 0i32;
    let mut end = 0;
    let chars: Vec<char> = input.chars().collect();

    for (i, &ch) in chars.iter().enumerate() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth == 0 {
                    end = i;
                    break;
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                end = i;
                break;
            }
            _ => {}
        }
        end = i + 1;
    }

    let text = input[..end].trim().trim_end_matches(',').trim();
    if text.is_empty() {
        return None;
    }
    Some(text.to_string())
}

/// Extract the edge filter traversal from SubgraphStrategy.
/// e.g., `SubgraphStrategy(edges: __.hasLabel("created"))`
pub(crate) fn extract_subgraph_edge_filter(
    input: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Option<Vec<GremlinStep>> {
    if !input.contains("SubgraphStrategy") {
        return None;
    }
    let start = input.find("SubgraphStrategy(")?;
    let after = &input[start + "SubgraphStrategy(".len()..];

    // Find "edges:" parameter
    let epos = after.find("edges:")?;
    let rest = after[epos + "edges:".len()..].trim_start();

    let traversal_text = extract_balanced_traversal(rest)?;
    parse_inner_traversal(&traversal_text, resolve_param).ok()
}

/// Inject SubgraphStrategy vertex filter steps after V() and vertex-producing navigation steps.
pub(crate) fn inject_subgraph_vertex_filter(
    steps: Vec<GremlinStep>,
    filter_steps: &[GremlinStep],
) -> Vec<GremlinStep> {
    let mut result = Vec::with_capacity(steps.len() * 2);

    for step in steps {
        let needs_vertex_filter = matches!(
            step,
            GremlinStep::V(_)
                | GremlinStep::Out(_)
                | GremlinStep::In(_)
                | GremlinStep::Both(_)
                | GremlinStep::InV
                | GremlinStep::OutV
                | GremlinStep::BothV
                | GremlinStep::OtherV
        );

        result.push(step);

        if needs_vertex_filter {
            result.push(GremlinStep::Where(filter_steps.to_vec()));
        }
    }

    result
}

/// Inject SubgraphStrategy edge filter steps after edge-producing steps.
/// For vertex-navigation steps (out/in/both), decompose them to use
/// edge traversal with filter: out() → outE().where(filter).inV()
pub(crate) fn inject_subgraph_edge_filter(
    steps: Vec<GremlinStep>,
    filter_steps: &[GremlinStep],
) -> Vec<GremlinStep> {
    let mut result = Vec::with_capacity(steps.len() * 3);

    for step in steps {
        match &step {
            // Edge-producing steps: inject filter after
            GremlinStep::E(_)
            | GremlinStep::OutE(_)
            | GremlinStep::InE(_)
            | GremlinStep::BothE(_) => {
                result.push(step);
                result.push(GremlinStep::Where(filter_steps.to_vec()));
            }
            // Vertex-navigation steps: decompose to edge traversal with filter
            GremlinStep::Out(labels) => {
                result.push(GremlinStep::OutE(labels.clone()));
                result.push(GremlinStep::Where(filter_steps.to_vec()));
                result.push(GremlinStep::InV);
            }
            GremlinStep::In(labels) => {
                result.push(GremlinStep::InE(labels.clone()));
                result.push(GremlinStep::Where(filter_steps.to_vec()));
                result.push(GremlinStep::OutV);
            }
            GremlinStep::Both(labels) => {
                result.push(GremlinStep::BothE(labels.clone()));
                result.push(GremlinStep::Where(filter_steps.to_vec()));
                result.push(GremlinStep::OtherV);
            }
            _ => {
                result.push(step);
            }
        }
    }

    result
}
