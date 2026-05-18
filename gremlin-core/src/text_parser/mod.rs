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

    // Detect withBulk(false) before stripping
    let with_bulk_false = input.contains("withBulk(false)");

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
    // ConnectiveStrategy: disabled when withoutStrategies(ConnectiveStrategy) is present
    let without_connective = input.contains("withoutStrategies(ConnectiveStrategy");
    // ReservedKeysVerificationStrategy(throwException: true, keys: {...})
    let reserved_keys = extract_reserved_keys_strategy(input);
    // SeedStrategy(seed: 999999) — extract seed value for deterministic random
    let seed_strategy = extract_seed_strategy(input);
    // SubgraphStrategy — extract vertex/edge filter traversals
    let subgraph_vertex_filter = extract_subgraph_vertex_filter(input, resolve_param);
    let subgraph_edge_filter = extract_subgraph_edge_filter(input, resolve_param);

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
    if let Some((init_val, merge_op)) = sack_init {
        steps.push(GremlinStep::WithSack(init_val, merge_op));
    }

    // Emit WithBulkFalse if present
    if with_bulk_false {
        steps.push(GremlinStep::WithBulkFalse);
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
    if without_connective {
        steps.push(GremlinStep::WithoutConnectiveStrategy);
    }
    if let Some(seed) = seed_strategy {
        steps.push(GremlinStep::SeedStrategy(seed));
    }

    for token in &tokens {
        parse_step_token(token, resolve_param, &mut steps)?;
    }

    // Apply SubgraphStrategy filters: inject vertex/edge filter steps
    if let Some(ref vf_steps) = subgraph_vertex_filter {
        steps = inject_subgraph_vertex_filter(steps, vf_steps);
    }
    if let Some(ref ef_steps) = subgraph_edge_filter {
        steps = inject_subgraph_edge_filter(steps, ef_steps);
    }

    Ok(steps)
}

mod literal;
mod predicate;
mod sack;
mod side_effect;
mod steps;
mod strategy;
mod tokenizer;
mod traversal;

#[cfg(test)]
mod tests;

use predicate::*;
use sack::*;
use side_effect::*;
use steps::*;
use strategy::*;
use tokenizer::*;

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
        parse_step_token(token, resolve_param, &mut steps)?;
    }
    Ok(steps)
}
