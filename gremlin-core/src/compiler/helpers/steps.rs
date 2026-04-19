//! Step-list analysis and structural transformations.
//!
//! Extracted from `helpers.rs` as part of the helpers/ submodule split.
//! All items are `pub(in crate::compiler)` and behave identically to their pre-split
//! definitions.

use std::collections::HashSet;

use cypher_core::planner::AggFunction;

use crate::bytecode::GremlinStep;

/// Check whether all steps in a branch are simple predicate-convertible steps.
pub(in crate::compiler) fn is_simple_predicate_steps(steps: &[GremlinStep]) -> bool {
    !steps.is_empty()
        && steps.iter().all(|s| {
            matches!(
                s,
                GremlinStep::Is(_)
                    | GremlinStep::Has(_, _)
                    | GremlinStep::HasPredicate(_, _)
                    | GremlinStep::HasLabel(_)
                    | GremlinStep::HasId(_)
                    | GremlinStep::HasIdPredicate(_)
                    | GremlinStep::Values(_)
                    | GremlinStep::Label
                    | GremlinStep::Id
                    | GremlinStep::Select(..)
            )
        })
}

/// Recursively check if any step in the sequence uses path tracking.
pub(in crate::compiler) fn steps_need_path_tracking(steps: &[GremlinStep]) -> bool {
    for step in steps {
        match step {
            GremlinStep::Path | GremlinStep::SimplePath | GremlinStep::CyclicPath => {
                return true;
            }
            GremlinStep::Tree(_) => {
                return true;
            }
            GremlinStep::Repeat(_, inner)
            | GremlinStep::FlatMap(inner)
            | GremlinStep::Map(inner)
            | GremlinStep::Not(inner)
            | GremlinStep::Where(inner)
                if steps_need_path_tracking(inner) =>
            {
                return true;
            }
            GremlinStep::And(branches)
            | GremlinStep::Or(branches)
            | GremlinStep::Union(branches) => {
                for branch in branches {
                    if steps_need_path_tracking(branch) {
                        return true;
                    }
                }
            }
            GremlinStep::Choose(if_branch, else_branch, _)
                if (steps_need_path_tracking(if_branch)
                    || steps_need_path_tracking(else_branch)) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Check if any step in the sequence uses select() with a Pop modifier.
pub(in crate::compiler) fn steps_need_label_stacks(steps: &[GremlinStep]) -> bool {
    for step in steps {
        match step {
            GremlinStep::Select(Some(_), _) => return true,
            GremlinStep::Repeat(_, inner)
            | GremlinStep::FlatMap(inner)
            | GremlinStep::Map(inner)
            | GremlinStep::Not(inner)
            | GremlinStep::Where(inner)
                if steps_need_label_stacks(inner) =>
            {
                return true;
            }
            GremlinStep::And(branches)
            | GremlinStep::Or(branches)
            | GremlinStep::Union(branches) => {
                for branch in branches {
                    if steps_need_label_stacks(branch) {
                        return true;
                    }
                }
            }
            GremlinStep::Choose(if_branch, else_branch, _)
                if (steps_need_label_stacks(if_branch) || steps_need_label_stacks(else_branch)) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Pre-scan a step sequence to collect all aggregate() key names and insert
/// them into `keys`. This allows within()/without() predicates that reference
/// an aggregate key defined LATER in the same step sequence (e.g. inside
/// repeat bodies) to resolve to __gremlin_agg_<key> at compile time rather
/// than treating the key as a literal string value.
pub(in crate::compiler) fn collect_aggregate_keys_prescan(
    steps: &[GremlinStep],
    keys: &mut HashSet<String>,
) {
    for step in steps {
        match step {
            GremlinStep::Aggregate(key) => {
                keys.insert(key.clone());
            }
            GremlinStep::Repeat(_, inner)
            | GremlinStep::FlatMap(inner)
            | GremlinStep::Map(inner)
            | GremlinStep::Not(inner)
            | GremlinStep::Where(inner) => {
                collect_aggregate_keys_prescan(inner, keys);
            }
            GremlinStep::And(branches)
            | GremlinStep::Or(branches)
            | GremlinStep::Union(branches) => {
                for branch in branches {
                    collect_aggregate_keys_prescan(branch, keys);
                }
            }
            GremlinStep::Choose(if_branch, else_branch, _) => {
                collect_aggregate_keys_prescan(if_branch, keys);
                collect_aggregate_keys_prescan(else_branch, keys);
            }
            _ => {}
        }
    }
}

/// Preprocess a step list to handle infix `and()`/`or()` with no arguments.
///
/// In Gremlin, `hasLabel("person").and().out("created")` is syntactic sugar for
/// `and(hasLabel("person"), out("created"))`. This function detects empty `And([])`
/// and `Or([])` steps and transforms the surrounding steps into proper branches.
///
/// Source steps (V, E, Inject) are preserved at the beginning, outside the
/// connective branches.  Or has lower precedence than And.
pub(in crate::compiler) fn preprocess_infix_connectives(steps: &[GremlinStep]) -> Vec<GremlinStep> {
    let has_infix = steps.iter().any(|s| match s {
        GremlinStep::And(b) if b.is_empty() => true,
        GremlinStep::Or(b) if b.is_empty() => true,
        _ => false,
    });
    if !has_infix {
        return steps.to_vec();
    }

    // Separate leading source steps from traversal steps.
    let source_end = steps
        .iter()
        .position(|s| {
            !matches!(
                s,
                GremlinStep::V(_) | GremlinStep::E(_) | GremlinStep::Inject(_)
            )
        })
        .unwrap_or(steps.len());

    let source_steps = &steps[..source_end];
    let traversal_steps = &steps[source_end..];

    // Re-check that the infix connective is in the traversal portion.
    let has_infix_in_traversal = traversal_steps.iter().any(|s| match s {
        GremlinStep::And(b) if b.is_empty() => true,
        GremlinStep::Or(b) if b.is_empty() => true,
        _ => false,
    });
    if !has_infix_in_traversal {
        return steps.to_vec();
    }

    // Split traversal steps at Or([]) first (lower precedence).
    let or_segments = split_at_empty_connective(traversal_steps, false);

    let mut result: Vec<GremlinStep> = source_steps.to_vec();
    if or_segments.len() > 1 {
        let or_branches: Vec<Vec<GremlinStep>> = or_segments
            .into_iter()
            .map(|segment| {
                let and_segments = split_at_empty_connective(&segment, true);
                if and_segments.len() > 1 {
                    vec![GremlinStep::And(and_segments)]
                } else {
                    segment
                }
            })
            .collect();
        result.push(GremlinStep::Or(or_branches));
    } else {
        let and_segments = split_at_empty_connective(traversal_steps, true);
        if and_segments.len() > 1 {
            result.push(GremlinStep::And(and_segments));
        } else {
            result.extend(traversal_steps.iter().cloned());
        }
    }
    result
}

/// Split a step list at empty `And([])` (is_and=true) or `Or([])` (is_and=false).
pub(in crate::compiler) fn split_at_empty_connective(
    steps: &[GremlinStep],
    is_and: bool,
) -> Vec<Vec<GremlinStep>> {
    let mut segments = Vec::new();
    let mut current_seg = Vec::new();
    for step in steps {
        let is_split = match step {
            GremlinStep::And(b) if b.is_empty() && is_and => true,
            GremlinStep::Or(b) if b.is_empty() && !is_and => true,
            _ => false,
        };
        if is_split {
            segments.push(std::mem::take(&mut current_seg));
        } else {
            current_seg.push(step.clone());
        }
    }
    segments.push(current_seg);
    segments
}

/// Split a trailing aggregation step (Count, Sum, Fold) from a list of Gremlin steps.
/// Returns the remaining steps (before the agg) and the detected aggregation function.
pub(in crate::compiler) fn split_trailing_agg(
    steps: &[GremlinStep],
) -> (&[GremlinStep], Option<AggFunction>) {
    if let Some(last) = steps.last() {
        match last {
            GremlinStep::Count => (&steps[..steps.len() - 1], Some(AggFunction::Count)),
            GremlinStep::Sum => (&steps[..steps.len() - 1], Some(AggFunction::Sum)),
            GremlinStep::Fold => (&steps[..steps.len() - 1], Some(AggFunction::Collect)),
            _ => (steps, None),
        }
    } else {
        (steps, None)
    }
}

pub(in crate::compiler) fn split_trailing_group_value_agg(
    steps: &[GremlinStep],
) -> (&[GremlinStep], Option<AggFunction>) {
    match steps.last() {
        Some(GremlinStep::Fold) => (steps, None),
        _ => split_trailing_agg(steps),
    }
}
