//! Free helper functions used by the Gremlin compiler.
//!
//! Extracted from `compiler.rs` to keep the main module under a manageable
//! line count. All items are `pub(super)` and behave identically to their
//! pre-split definitions.

use std::collections::{HashMap, HashSet};

use cypher_core::ast::{BinaryOp, Expr, Identifier, Label, UnaryOp};
use cypher_core::planner::{AggFunction, Operator};
use cypher_core::types::Value;
use smol_str::SmolStr;

use crate::bytecode::{GremlinStep, Predicate};
use crate::types::GremlinValue;

/// Check whether all steps in a branch are simple predicate-convertible steps.
pub(super) fn is_simple_predicate_steps(steps: &[GremlinStep]) -> bool {
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
pub(super) fn steps_need_path_tracking(steps: &[GremlinStep]) -> bool {
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
pub(super) fn steps_need_label_stacks(steps: &[GremlinStep]) -> bool {
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
pub(super) fn collect_aggregate_keys_prescan(steps: &[GremlinStep], keys: &mut HashSet<String>) {
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
pub(super) fn preprocess_infix_connectives(steps: &[GremlinStep]) -> Vec<GremlinStep> {
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
pub(super) fn split_at_empty_connective(
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

/// Inject a `__sack` column into the lowest-level scan/single-row operator
/// by wrapping it in an Apply that adds the initialization value.
pub(super) fn inject_sack_init(op: &mut Operator, init_expr: &Expr) {
    match op {
        Operator::Filter { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Distinct { child, .. }
        | Operator::GremlinAggregate { child, .. }
        | Operator::Cap { child, .. }
        | Operator::Repeat { child, .. }
        | Operator::Project { child, .. }
        | Operator::GremlinGroup { child, .. }
        | Operator::GremlinSelect { child, .. }
        | Operator::Choose { child, .. }
        | Operator::ChooseMap { child, .. }
        | Operator::Coalesce { child, .. }
        | Operator::Expand { child, .. }
        | Operator::GremlinSideEffect { child, .. }
        | Operator::GremlinPath { child, .. }
        | Operator::GremlinMatch { child, .. }
        | Operator::GremlinSimplePath { child, .. }
        | Operator::GremlinCyclicPath { child, .. }
        | Operator::GremlinIndex { child, .. }
        | Operator::GremlinProject { child, .. }
        | Operator::GremlinMapSlice { child, .. }
        | Operator::ForEach { child, .. }
        | Operator::VarLenExpand { child, .. }
        | Operator::Aggregate { child, .. }
        | Operator::GremlinBarrier { child, .. } => {
            inject_sack_init(child, init_expr);
        }
        Operator::Apply { outer, .. }
        | Operator::LeftOuterApply {
            preserved: outer, ..
        } => {
            inject_sack_init(outer, init_expr);
        }
        Operator::Union { left, right, .. } => {
            inject_sack_init(left, init_expr);
            inject_sack_init(right, init_expr);
        }
        // Leaf operators: wrap in Apply that adds __sack
        Operator::NodeScan { .. }
        | Operator::IndexScan { .. }
        | Operator::NodeByIdSeek { .. }
        | Operator::SingleRow
        | Operator::Unwind { .. } => {
            let original = std::mem::replace(op, Operator::SingleRow);
            *op = Operator::Apply {
                outer: Box::new(original),
                inner: Box::new(Operator::Project {
                    items: vec![(init_expr.clone(), Some(Identifier::new("__sack")))],
                    child: Box::new(Operator::SingleRow),
                }),
            };
        }
        // Operators with optional child: recurse if child exists
        Operator::Create { child, .. }
        | Operator::Merge { child, .. }
        | Operator::Call { child, .. } => {
            if let Some(ch) = child {
                inject_sack_init(ch, init_expr);
            }
        }
        // Remaining operators with a required child
        Operator::MergeRelationship { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::Delete { child, .. }
        | Operator::AssemblePath { child, .. }
        | Operator::ProduceResults { child, .. } => {
            inject_sack_init(child, init_expr);
        }
        _ => {}
    }
}

/// Inject a vertex scan into the child chain of an operator (typically Create)
/// so that the scan's alias is available in every row when the operator executes.
/// Uses Apply with scan as outer and original operator as inner.
pub(super) fn inject_scan_into_child(op: Operator, scan: Operator) -> Operator {
    match op {
        Operator::Create { patterns, child } => {
            let new_child = match child {
                Some(ch) => Some(Box::new(Operator::Apply {
                    outer: Box::new(scan),
                    inner: ch,
                })),
                None => Some(Box::new(scan)),
            };
            Operator::Create {
                patterns,
                child: new_child,
            }
        }
        Operator::Set { items, child } => {
            // Set wrapping Create: inject into Set's child
            Operator::Set {
                items,
                child: Box::new(inject_scan_into_child(*child, scan)),
            }
        }
        other => {
            // Fallback: Apply with scan as outer
            Operator::Apply {
                outer: Box::new(scan),
                inner: Box::new(other),
            }
        }
    }
}

/// Recursively rewrite a placeholder alias in a `Create` operator's edge specs.
pub(super) fn rewrite_create_alias(op: &mut Operator, placeholder: &str, replacement: &Identifier) {
    match op {
        Operator::Create { patterns, .. } => {
            for pat in patterns {
                for edge in &mut pat.edges {
                    // edge is (alias, src, dst, rel_type, dir, props)
                    if edge.1.as_str() == placeholder {
                        edge.1 = replacement.clone();
                    }
                    if edge.2.as_str() == placeholder {
                        edge.2 = replacement.clone();
                    }
                }
            }
        }
        Operator::MergeRelationship {
            src_alias,
            dst_alias,
            child,
            ..
        } => {
            if src_alias.as_str() == placeholder {
                *src_alias = replacement.clone();
            }
            if dst_alias.as_str() == placeholder {
                *dst_alias = replacement.clone();
            }
            rewrite_create_alias(child, placeholder, replacement);
        }
        Operator::Apply { outer, inner } => {
            rewrite_create_alias(outer, placeholder, replacement);
            rewrite_create_alias(inner, placeholder, replacement);
        }
        Operator::Set { child, .. } => {
            rewrite_create_alias(child, placeholder, replacement);
        }
        _ => {}
    }
}

/// Convert a `GremlinValue` to an `Expr` literal for use in ID-comparison
/// contexts (V(id), E(id), hasId(id)).
///
/// TinkerPop allows string representations of numeric IDs: when a vertex
/// is looked up as `V("1")`, the string `"1"` should match the numeric node id
/// `1`. This helper tries to parse the string as an i64 and emits
/// `Expr::Integer` so the downstream filter `id(v) == 1` succeeds.
pub(super) fn gremlin_value_to_id_expr(gv: &GremlinValue) -> Expr {
    if let GremlinValue::String(s) = gv {
        if let Ok(n) = s.parse::<i64>() {
            return Expr::Integer(n);
        }
    }
    gremlin_value_to_expr(gv)
}

pub(super) fn is_gremlin_id_key(key: &str) -> bool {
    matches!(key, "~id" | "T.id" | "t[id]")
}

pub(super) fn gremlin_has_key_expr(key: &str, current_alias: &Identifier) -> Expr {
    if is_gremlin_id_key(key) {
        Expr::FunctionCall {
            name: SmolStr::new("id"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else {
        Expr::Property(
            Box::new(Expr::Variable(current_alias.clone())),
            SmolStr::new(key),
        )
    }
}

/// Convert a `GremlinValue` to an `Expr` literal.
pub(super) fn gremlin_value_to_expr(gv: &GremlinValue) -> Expr {
    match gv {
        GremlinValue::Null => Expr::Null,
        GremlinValue::Bool(b) => Expr::Bool(*b),
        GremlinValue::Integer(i) => Expr::Integer(*i),
        GremlinValue::Float(f) => Expr::Float(*f),
        GremlinValue::String(s) => {
            if let Some(iso) = s.strip_prefix("__gremlin_dt__:") {
                Expr::FunctionCall {
                    name: SmolStr::new("gremlin_make_datetime"),
                    args: vec![Expr::Str(iso.to_string().into_boxed_str())],
                    distinct: false,
                }
            } else if let Some(uuid) = s.strip_prefix("__gremlin_uuid__:") {
                Expr::FunctionCall {
                    name: SmolStr::new("gremlin_make_uuid"),
                    args: vec![Expr::Str(uuid.to_string().into_boxed_str())],
                    distinct: false,
                }
            } else {
                Expr::Str(s.clone().into_boxed_str())
            }
        }
        GremlinValue::List(items) => Expr::List(items.iter().map(gremlin_value_to_expr).collect()),
        GremlinValue::Map(entries) => Expr::MapLiteral(
            entries
                .iter()
                .map(|(k, v)| (SmolStr::new(k), gremlin_value_to_expr(v)))
                .collect(),
        ),
        GremlinValue::Vertex { id, .. } => Expr::Integer(*id),
        GremlinValue::Edge { id, .. } => Expr::Integer(*id),
        GremlinValue::Path { .. } => Expr::Null,
    }
}

/// Convert a `GremlinValue` to a core `Value` for use in ChooseMap option keys.
pub(super) fn gremlin_value_to_value(gv: &GremlinValue) -> Value {
    match gv {
        GremlinValue::Null => Value::Null,
        GremlinValue::Bool(b) => Value::Bool(*b),
        GremlinValue::Integer(i) => Value::Int(*i),
        GremlinValue::Float(f) => Value::Float(*f),
        GremlinValue::String(s) => Value::Str(s.clone().into_boxed_str()),
        GremlinValue::List(items) => {
            Value::List(items.iter().map(gremlin_value_to_value).collect())
        }
        GremlinValue::Map(_)
        | GremlinValue::Vertex { .. }
        | GremlinValue::Edge { .. }
        | GremlinValue::Path { .. } => Value::Null,
    }
}

/// Split a trailing aggregation step (Count, Sum, Fold) from a list of Gremlin steps.
/// Returns the remaining steps (before the agg) and the detected aggregation function.
pub(super) fn split_trailing_agg(steps: &[GremlinStep]) -> (&[GremlinStep], Option<AggFunction>) {
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

pub(super) fn split_trailing_group_value_agg(
    steps: &[GremlinStep],
) -> (&[GremlinStep], Option<AggFunction>) {
    match steps.last() {
        Some(GremlinStep::Fold) => (steps, None),
        _ => split_trailing_agg(steps),
    }
}

/// Build an `Expr` from a `by(key)` modulator key string and the current alias.
/// Handles T.id, T.label, Column.keys/values, common traversal patterns, and property names.
pub(super) fn make_by_expr(key: &str, current_alias: &Identifier) -> Expr {
    if key == "~id" || key == "T.id" || key == "t[id]" {
        Expr::FunctionCall {
            name: SmolStr::new("id"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if key == "~label"
        || key == "T.label"
        || key == "t[label]"
        || key == "__.label()"
        || key == "label()"
    {
        Expr::FunctionCall {
            name: SmolStr::new("gremlin_single_label"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if let Some(prop) = key
        .strip_prefix("__.values(")
        .or_else(|| key.strip_prefix("values("))
        .and_then(|s| s.strip_suffix(')'))
    {
        // by(values("name")) or by(__.values("name")) → property access
        let prop = prop.trim_matches(|c| c == '\'' || c == '"');
        Expr::Property(
            Box::new(Expr::Variable(current_alias.clone())),
            SmolStr::new(prop),
        )
    } else if key == "~key" || key == "T.key" || key == "t[key]" {
        Expr::FunctionCall {
            name: SmolStr::new("gremlin_property_key"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if key == "~value" || key == "T.value" || key == "t[value]" {
        Expr::FunctionCall {
            name: SmolStr::new("gremlin_property_value"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if key == "Column.keys" || key == "column.keys" {
        // Extract the key from a single-entry Map produced by unfold() on a Map.
        // gremlin_unfold_map_entry_key returns a scalar Str for single-entry maps.
        Expr::FunctionCall {
            name: SmolStr::new("gremlin_unfold_map_entry_key"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if key == "Column.values" || key == "column.values" {
        // Extract the value from a single-entry Map produced by unfold() on a Map.
        Expr::FunctionCall {
            name: SmolStr::new("gremlin_unfold_map_entry_value"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else {
        Expr::Property(
            Box::new(Expr::Variable(current_alias.clone())),
            SmolStr::new(key),
        )
    }
}

/// Rewrite a sack assignment expression, replacing Expr::Variable references
/// that match `target` with `replacement`. Used by `.by(key)` after `sack(op)`.
pub(super) fn rewrite_sack_by(expr: &mut Expr, target: &Identifier, replacement: &Expr) {
    match expr {
        Expr::Variable(id) if id == target => {
            *expr = replacement.clone();
        }
        Expr::BinaryOp { left, right, .. } => {
            rewrite_sack_by(left, target, replacement);
            rewrite_sack_by(right, target, replacement);
        }
        _ => {}
    }
}

/// Convert a single GremlinStep into a filter predicate Expr.
pub(super) fn step_to_predicate(step: &GremlinStep, alias: &Identifier) -> Expr {
    match step {
        GremlinStep::Has(key, Some(val)) => Expr::BinaryOp {
            op: BinaryOp::Eq,
            left: Box::new(Expr::Property(
                Box::new(Expr::Variable(alias.clone())),
                SmolStr::new(key),
            )),
            right: Box::new(gremlin_value_to_expr(val)),
        },
        GremlinStep::Has(key, None) => Expr::IsNotNull(Box::new(Expr::Property(
            Box::new(Expr::Variable(alias.clone())),
            SmolStr::new(key),
        ))),
        GremlinStep::HasPredicate(key, pred) => {
            let prop_expr =
                Expr::Property(Box::new(Expr::Variable(alias.clone())), SmolStr::new(key));
            // has(key, pred) means: property exists AND satisfies predicate.
            // Wrap in AND(IsNotNull(prop), pred(prop)) to properly handle
            // missing properties (returns false instead of null).
            let pred_expr = predicate_to_prop_expr(pred, prop_expr.clone());
            Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(Expr::IsNotNull(Box::new(prop_expr))),
                right: Box::new(pred_expr),
            }
        }
        GremlinStep::HasLabel(labels) if !labels.is_empty() => labels
            .iter()
            .map(|l| {
                Expr::HasLabel(
                    Box::new(Expr::Variable(alias.clone())),
                    Label::new(l.as_str()),
                )
            })
            .reduce(|a, b| Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(a),
                right: Box::new(b),
            })
            .unwrap(),
        GremlinStep::Is(pred) => predicate_to_expr(pred, alias),
        GremlinStep::HasId(ids) if !ids.is_empty() => {
            let id_fn = Expr::FunctionCall {
                name: SmolStr::new("id"),
                args: vec![Expr::Variable(alias.clone())],
                distinct: false,
            };
            // Flatten list parameters: hasId(id1, [id2, id3]) → hasId(id1, id2, id3)
            let mut flat_ids = Vec::new();
            for id in ids {
                match id {
                    GremlinValue::List(inner) => flat_ids.extend(inner.iter().cloned()),
                    other => flat_ids.push(other.clone()),
                }
            }
            if flat_ids.len() == 1 {
                Expr::BinaryOp {
                    op: BinaryOp::Eq,
                    left: Box::new(id_fn),
                    right: Box::new(gremlin_value_to_expr(&flat_ids[0])),
                }
            } else {
                let id_exprs: Vec<Expr> = flat_ids.iter().map(gremlin_value_to_expr).collect();
                Expr::BinaryOp {
                    op: BinaryOp::In,
                    left: Box::new(id_fn),
                    right: Box::new(Expr::List(id_exprs)),
                }
            }
        }
        GremlinStep::HasIdPredicate(pred) => {
            let id_fn = Expr::FunctionCall {
                name: SmolStr::new("id"),
                args: vec![Expr::Variable(alias.clone())],
                distinct: false,
            };
            predicate_to_prop_expr(pred, id_fn)
        }
        // Non-filter steps produce vacuous truth. Complex sub-traversals
        // are routed through Apply+Count existence checks by callers
        // (Where/Not) before reaching this function.
        _ => Expr::Bool(true),
    }
}

/// Convert a list of GremlinSteps into a filter predicate.
/// Handles multi-step traversals like `values("age").is(P.gt(30))`.
pub(super) fn steps_to_predicate(
    steps: &[GremlinStep],
    alias: &Identifier,
    alias_map: &HashMap<String, Identifier>,
    aggregate_keys: &HashSet<String>,
) -> Expr {
    if steps.is_empty() {
        return Expr::Bool(true);
    }

    // Single step: delegate to step_to_predicate
    if steps.len() == 1 {
        // Handle Or/And/Not at the single-step level too
        match &steps[0] {
            GremlinStep::Or(branches) => {
                return branches
                    .iter()
                    .map(|b| steps_to_predicate(b, alias, alias_map, aggregate_keys))
                    .reduce(|a, b| Expr::BinaryOp {
                        op: BinaryOp::Or,
                        left: Box::new(a),
                        right: Box::new(b),
                    })
                    .unwrap_or(Expr::Bool(false));
            }
            GremlinStep::And(branches) => {
                return branches
                    .iter()
                    .map(|b| steps_to_predicate(b, alias, alias_map, aggregate_keys))
                    .reduce(|a, b| Expr::BinaryOp {
                        op: BinaryOp::And,
                        left: Box::new(a),
                        right: Box::new(b),
                    })
                    .unwrap_or(Expr::Bool(true));
            }
            GremlinStep::Not(inner) => {
                return Expr::UnaryOp {
                    op: UnaryOp::Not,
                    expr: Box::new(steps_to_predicate(inner, alias, alias_map, aggregate_keys)),
                };
            }
            _ => {}
        }
        // Single Is(pred) where the pred argument is a string label reference:
        // where(P.eq("b")) → compare current value against labeled value "b"
        if let GremlinStep::Is(pred) = &steps[0] {
            // Handle Not(inner) by unwrapping: resolve the inner predicate's
            // label reference and then negate the result.
            let (negated, inner_pred) = match pred {
                Predicate::Not(inner) => (true, inner.as_ref()),
                _ => (false, pred),
            };
            if let Some(label_expr) = predicate_label_ref(inner_pred, alias_map, aggregate_keys) {
                let result = match inner_pred {
                    // within("key") / without("key") — membership test against the
                    // stored list.  within → alias IN key; without → NOT(alias IN key).
                    Predicate::Within(_) | Predicate::Without(_) => {
                        let is_without = matches!(inner_pred, Predicate::Without(_));
                        let cmp = Expr::BinaryOp {
                            op: BinaryOp::In,
                            left: Box::new(Expr::Variable(alias.clone())),
                            right: Box::new(label_expr),
                        };
                        // Negate for Without or when the outer predicate was negated
                        // (XOR so double-negation resolves correctly).
                        if is_without ^ negated {
                            Expr::UnaryOp {
                                op: UnaryOp::Not,
                                expr: Box::new(cmp),
                            }
                        } else {
                            cmp
                        }
                    }
                    // eq("a") / neq("a") / gt("a") etc. — direct comparison against
                    // the labeled value.
                    _ => {
                        let op = predicate_to_binary_op(inner_pred);
                        let cmp = Expr::BinaryOp {
                            op,
                            left: Box::new(Expr::Variable(alias.clone())),
                            right: Box::new(label_expr),
                        };
                        if negated {
                            Expr::UnaryOp {
                                op: UnaryOp::Not,
                                expr: Box::new(cmp),
                            }
                        } else {
                            cmp
                        }
                    }
                };
                return result;
            }
            // Also try predicate_has_label_refs + predicate_to_label_aware_expr
            // for compound predicates like P.not(P.neq("a"))
            if predicate_has_label_refs(pred, alias_map, aggregate_keys) {
                return predicate_to_label_aware_expr(
                    pred,
                    &Expr::Variable(alias.clone()),
                    alias_map,
                    aggregate_keys,
                );
            }
        }
        return step_to_predicate(&steps[0], alias);
    }

    // Two-step pattern: Values(["key"]) + Is(pred) → property predicate
    if steps.len() == 2 {
        // loops("label").is(pred) and loops().is(pred) must target the
        // executor-injected loop counter columns, not the current traverser.
        if let (GremlinStep::Loops(label), GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            let loop_var = match label {
                Some(l) => Identifier::new(format!("__gremlin_loop_count_{l}")),
                None => Identifier::new("__gremlin_loop_count"),
            };
            return predicate_to_expr(pred, &loop_var);
        }
        if let (GremlinStep::Values(keys), GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            if keys.len() == 1 {
                let prop_expr = Expr::Property(
                    Box::new(Expr::Variable(alias.clone())),
                    SmolStr::new(&keys[0]),
                );
                return predicate_to_prop_expr(pred, prop_expr);
            }
        }
        // Label + Is(Eq("person")) → hasLabel("person")
        if let (GremlinStep::Label, GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            match pred {
                Predicate::Eq(GremlinValue::String(label)) => {
                    return Expr::HasLabel(
                        Box::new(Expr::Variable(alias.clone())),
                        Label::new(label.as_str()),
                    );
                }
                _ => {
                    // For other predicates on label, use function call
                    let label_expr = Expr::FunctionCall {
                        name: SmolStr::new("labels"),
                        args: vec![Expr::Variable(alias.clone())],
                        distinct: false,
                    };
                    return predicate_to_prop_expr(pred, label_expr);
                }
            }
        }
        // Id + Is(pred) → id predicate
        if let (GremlinStep::Id, GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            let id_expr = Expr::FunctionCall {
                name: SmolStr::new("id"),
                args: vec![Expr::Variable(alias.clone())],
                distinct: false,
            };
            return predicate_to_prop_expr(pred, id_expr);
        }
        // Two-step pattern: Select(["label"]) + Is(pred) → labeled value comparison
        // This handles where("a", P.eq("b"))
        if let (GremlinStep::Select(_, keys), GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            if keys.len() == 1 {
                let src_expr = if keys[0] == "keys" || keys[0] == "Column.keys" {
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_unfold_map_entry_key"),
                        args: vec![Expr::Variable(alias.clone())],
                        distinct: false,
                    }
                } else if keys[0] == "values" || keys[0] == "Column.values" {
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_unfold_map_entry_value"),
                        args: vec![Expr::Variable(alias.clone())],
                        distinct: false,
                    }
                } else {
                    Expr::Variable(
                        alias_map
                            .get(&keys[0])
                            .cloned()
                            .unwrap_or_else(|| Identifier::new(keys[0].clone())),
                    )
                };
                if let Some(label_expr) = predicate_label_ref(pred, alias_map, aggregate_keys) {
                    let op = predicate_to_binary_op(pred);
                    return Expr::BinaryOp {
                        op,
                        left: Box::new(src_expr.clone()),
                        right: Box::new(label_expr),
                    };
                }
                if let Some(column_name) = predicate_string_ref(pred) {
                    let op = predicate_to_binary_op(pred);
                    return Expr::BinaryOp {
                        op,
                        left: Box::new(src_expr.clone()),
                        right: Box::new(Expr::Variable(Identifier::new(column_name))),
                    };
                }
                // For compound predicates (Or/And/Not) that reference labels,
                // build the expression resolving each label to its variable.
                if predicate_has_label_refs(pred, alias_map, aggregate_keys) {
                    return predicate_to_label_aware_expr(
                        pred,
                        &src_expr,
                        alias_map,
                        aggregate_keys,
                    );
                }
                if let Expr::Variable(src_var) = src_expr {
                    return predicate_to_expr(pred, &src_var);
                }
                return predicate_to_prop_expr(pred, src_expr);
            }
        }
    }

    // Multi-step patterns: try to build AND of individual predicates
    let mut preds = Vec::new();
    for step in steps {
        // Handle Or/And/Not by recursing into steps_to_predicate
        let pred = match step {
            GremlinStep::Or(branches) => branches
                .iter()
                .map(|b| steps_to_predicate(b, alias, alias_map, aggregate_keys))
                .reduce(|a, b| Expr::BinaryOp {
                    op: BinaryOp::Or,
                    left: Box::new(a),
                    right: Box::new(b),
                })
                .unwrap_or(Expr::Bool(false)),
            GremlinStep::And(branches) => branches
                .iter()
                .map(|b| steps_to_predicate(b, alias, alias_map, aggregate_keys))
                .reduce(|a, b| Expr::BinaryOp {
                    op: BinaryOp::And,
                    left: Box::new(a),
                    right: Box::new(b),
                })
                .unwrap_or(Expr::Bool(true)),
            GremlinStep::Not(inner) => {
                let inner_pred = steps_to_predicate(inner, alias, alias_map, aggregate_keys);
                Expr::UnaryOp {
                    op: UnaryOp::Not,
                    expr: Box::new(inner_pred),
                }
            }
            _ => step_to_predicate(step, alias),
        };
        if pred != Expr::Bool(true) {
            preds.push(pred);
        }
    }

    if preds.is_empty() {
        return Expr::Bool(true);
    }

    preds
        .into_iter()
        .reduce(|a, b| Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(a),
            right: Box::new(b),
        })
        .unwrap_or(Expr::Bool(true))
}

/// Extract predicate operator name and value expression for all()/any()/none() steps.
pub(super) fn predicate_to_args(pred: &Predicate) -> (Box<str>, Expr) {
    match pred {
        Predicate::Eq(v) => ("eq".into(), gremlin_value_to_expr(v)),
        Predicate::Neq(v) => ("neq".into(), gremlin_value_to_expr(v)),
        Predicate::Gt(v) => ("gt".into(), gremlin_value_to_expr(v)),
        Predicate::Gte(v) => ("gte".into(), gremlin_value_to_expr(v)),
        Predicate::Lt(v) => ("lt".into(), gremlin_value_to_expr(v)),
        Predicate::Lte(v) => ("lte".into(), gremlin_value_to_expr(v)),
        Predicate::Within(vals) => (
            "within".into(),
            Expr::List(vals.iter().map(gremlin_value_to_expr).collect()),
        ),
        Predicate::Without(vals) => (
            "without".into(),
            Expr::List(vals.iter().map(gremlin_value_to_expr).collect()),
        ),
        Predicate::Between(low, high) => (
            "within".into(),
            Expr::List(vec![
                gremlin_value_to_expr(low),
                gremlin_value_to_expr(high),
            ]),
        ),
        Predicate::Inside(low, high) => (
            "within".into(),
            Expr::List(vec![
                gremlin_value_to_expr(low),
                gremlin_value_to_expr(high),
            ]),
        ),
        _ => ("neq".into(), Expr::Null),
    }
}

/// Convert a `Predicate` to an `Expr`.
pub(super) fn predicate_to_expr(pred: &Predicate, alias: &Identifier) -> Expr {
    let var = Expr::Variable(alias.clone());
    match pred {
        Predicate::Eq(v) => {
            // Gremlin: P.eq(null) means IS NULL (unlike Cypher where null=null is null)
            if matches!(v, GremlinValue::Null) {
                return Expr::IsNull(Box::new(var));
            }
            // Gremlin: null.eq(non_null) is false, not null.
            // Use (var IS NOT NULL AND var == val) to prevent null from passing.
            Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(Expr::IsNotNull(Box::new(Expr::Variable(alias.clone())))),
                right: Box::new(Expr::BinaryOp {
                    op: BinaryOp::Eq,
                    left: Box::new(var),
                    right: Box::new(gremlin_value_to_expr(v)),
                }),
            }
        }
        Predicate::Neq(v) => {
            if matches!(v, GremlinValue::Null) {
                return Expr::IsNotNull(Box::new(var));
            }
            // Gremlin: null != non_null is true (pass through).
            // Use (var IS NULL OR var != val) to handle null propagation.
            Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(Expr::IsNull(Box::new(Expr::Variable(alias.clone())))),
                right: Box::new(Expr::BinaryOp {
                    op: BinaryOp::Neq,
                    left: Box::new(var),
                    right: Box::new(gremlin_value_to_expr(v)),
                }),
            }
        }
        Predicate::Gt(v) => {
            if matches!(v, GremlinValue::Null) {
                return Expr::Bool(false);
            }
            Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(var),
                right: Box::new(gremlin_value_to_expr(v)),
            }
        }
        Predicate::Gte(v) => {
            // Gremlin: P.gte(null) on null value should be true
            if matches!(v, GremlinValue::Null) {
                return Expr::IsNull(Box::new(var));
            }
            Expr::BinaryOp {
                op: BinaryOp::Gte,
                left: Box::new(var),
                right: Box::new(gremlin_value_to_expr(v)),
            }
        }
        Predicate::Lt(v) => {
            // Gremlin: P.lt(null) always false (nothing is strictly less than null)
            if matches!(v, GremlinValue::Null) {
                return Expr::Bool(false);
            }
            Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(var),
                right: Box::new(gremlin_value_to_expr(v)),
            }
        }
        Predicate::Lte(v) => {
            if matches!(v, GremlinValue::Null) {
                return Expr::IsNull(Box::new(var));
            }
            Expr::BinaryOp {
                op: BinaryOp::Lte,
                left: Box::new(var),
                right: Box::new(gremlin_value_to_expr(v)),
            }
        }
        Predicate::Within(vals) => Expr::BinaryOp {
            op: BinaryOp::In,
            left: Box::new(var),
            right: Box::new(Expr::List(vals.iter().map(gremlin_value_to_expr).collect())),
        },
        Predicate::Without(vals) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::BinaryOp {
                op: BinaryOp::In,
                left: Box::new(var),
                right: Box::new(Expr::List(vals.iter().map(gremlin_value_to_expr).collect())),
            }),
        },
        Predicate::Between(low, high) => Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Gte,
                left: Box::new(Expr::Variable(alias.clone())),
                right: Box::new(gremlin_value_to_expr(low)),
            }),
            right: Box::new(Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(Expr::Variable(alias.clone())),
                right: Box::new(gremlin_value_to_expr(high)),
            }),
        },
        // inside(low, high) → low < x < high (exclusive on both ends)
        Predicate::Inside(low, high) => Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(Expr::Variable(alias.clone())),
                right: Box::new(gremlin_value_to_expr(low)),
            }),
            right: Box::new(Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(Expr::Variable(alias.clone())),
                right: Box::new(gremlin_value_to_expr(high)),
            }),
        },
        // Text predicates reuse the same pattern as predicate_to_prop_expr
        Predicate::Containing(_)
        | Predicate::NotContaining(_)
        | Predicate::StartingWith(_)
        | Predicate::NotStartingWith(_)
        | Predicate::EndingWith(_)
        | Predicate::NotEndingWith(_)
        | Predicate::Regex(_)
        | Predicate::NotRegex(_)
        | Predicate::IsType(_)
        | Predicate::And(_)
        | Predicate::Or(_)
        | Predicate::Not(_) => predicate_to_prop_expr(pred, var),
    }
}

/// Apply a predicate to a property expression (for `has("key", P.gt(30))`).
pub(super) fn predicate_to_prop_expr(pred: &Predicate, prop_expr: Expr) -> Expr {
    match pred {
        Predicate::Eq(v) => Expr::BinaryOp {
            op: BinaryOp::Eq,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Neq(v) => Expr::BinaryOp {
            op: BinaryOp::Neq,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Gt(v) => Expr::BinaryOp {
            op: BinaryOp::Gt,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Gte(v) => Expr::BinaryOp {
            op: BinaryOp::Gte,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Lt(v) => Expr::BinaryOp {
            op: BinaryOp::Lt,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Lte(v) => Expr::BinaryOp {
            op: BinaryOp::Lte,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Within(vals) => Expr::BinaryOp {
            op: BinaryOp::In,
            left: Box::new(prop_expr),
            right: Box::new(Expr::List(vals.iter().map(gremlin_value_to_expr).collect())),
        },
        Predicate::Without(vals) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::BinaryOp {
                op: BinaryOp::In,
                left: Box::new(prop_expr),
                right: Box::new(Expr::List(vals.iter().map(gremlin_value_to_expr).collect())),
            }),
        },
        Predicate::Between(low, high) => Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Gte,
                left: Box::new(prop_expr.clone()),
                right: Box::new(gremlin_value_to_expr(low)),
            }),
            right: Box::new(Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(prop_expr),
                right: Box::new(gremlin_value_to_expr(high)),
            }),
        },
        // inside(low, high) → low < x < high (exclusive on both ends)
        Predicate::Inside(low, high) => Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(prop_expr.clone()),
                right: Box::new(gremlin_value_to_expr(low)),
            }),
            right: Box::new(Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(prop_expr),
                right: Box::new(gremlin_value_to_expr(high)),
            }),
        },
        Predicate::Containing(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_containing"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::NotContaining(v) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::FunctionCall {
                name: SmolStr::new("gremlin_containing"),
                args: vec![prop_expr, gremlin_value_to_expr(v)],
                distinct: false,
            }),
        },
        Predicate::StartingWith(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_startingWith"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::NotStartingWith(v) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::FunctionCall {
                name: SmolStr::new("gremlin_startingWith"),
                args: vec![prop_expr, gremlin_value_to_expr(v)],
                distinct: false,
            }),
        },
        Predicate::EndingWith(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_endingWith"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::NotEndingWith(v) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::FunctionCall {
                name: SmolStr::new("gremlin_endingWith"),
                args: vec![prop_expr, gremlin_value_to_expr(v)],
                distinct: false,
            }),
        },
        Predicate::Regex(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_regex"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::NotRegex(v) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::FunctionCall {
                name: SmolStr::new("gremlin_regex"),
                args: vec![prop_expr, gremlin_value_to_expr(v)],
                distinct: false,
            }),
        },
        Predicate::IsType(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_istype"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::And(preds) => {
            let mut iter = preds.iter();
            let first = predicate_to_prop_expr(iter.next().unwrap(), prop_expr.clone());
            iter.fold(first, |acc, p| Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(acc),
                right: Box::new(predicate_to_prop_expr(p, prop_expr.clone())),
            })
        }
        Predicate::Or(preds) => {
            let mut iter = preds.iter();
            let first = predicate_to_prop_expr(iter.next().unwrap(), prop_expr.clone());
            iter.fold(first, |acc, p| Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(acc),
                right: Box::new(predicate_to_prop_expr(p, prop_expr.clone())),
            })
        }
        Predicate::Not(inner) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(predicate_to_prop_expr(inner, prop_expr)),
        },
    }
}

pub(super) fn predicate_string_ref(pred: &Predicate) -> Option<&str> {
    match pred {
        Predicate::Eq(GremlinValue::String(s))
        | Predicate::Neq(GremlinValue::String(s))
        | Predicate::Gt(GremlinValue::String(s))
        | Predicate::Gte(GremlinValue::String(s))
        | Predicate::Lt(GremlinValue::String(s))
        | Predicate::Lte(GremlinValue::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Check if any leaf predicate in a compound predicate references a known label.
pub(super) fn predicate_has_label_refs(
    pred: &Predicate,
    alias_map: &HashMap<String, Identifier>,
    aggregate_keys: &HashSet<String>,
) -> bool {
    match pred {
        Predicate::Eq(GremlinValue::String(s))
        | Predicate::Neq(GremlinValue::String(s))
        | Predicate::Gt(GremlinValue::String(s))
        | Predicate::Gte(GremlinValue::String(s))
        | Predicate::Lt(GremlinValue::String(s))
        | Predicate::Lte(GremlinValue::String(s)) => alias_map.contains_key(s.as_str()),
        // within("key") / without("key") with a single string arg that names
        // an aggregate side-effect or alias counts as a label reference.
        Predicate::Within(vals) | Predicate::Without(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                alias_map.contains_key(s.as_str()) || aggregate_keys.contains(s.as_str())
            } else {
                false
            }
        }
        Predicate::Or(preds) | Predicate::And(preds) => preds
            .iter()
            .any(|p| predicate_has_label_refs(p, alias_map, aggregate_keys)),
        Predicate::Not(inner) => predicate_has_label_refs(inner, alias_map, aggregate_keys),
        _ => false,
    }
}

/// Build a comparison expression for a compound predicate where each leaf
/// may reference a different label or aggregate side-effect key.
/// Resolves each label/key to its variable.
pub(super) fn predicate_to_label_aware_expr(
    pred: &Predicate,
    left: &Expr,
    alias_map: &HashMap<String, Identifier>,
    aggregate_keys: &HashSet<String>,
) -> Expr {
    match pred {
        Predicate::Eq(GremlinValue::String(s))
        | Predicate::Neq(GremlinValue::String(s))
        | Predicate::Gt(GremlinValue::String(s))
        | Predicate::Gte(GremlinValue::String(s))
        | Predicate::Lt(GremlinValue::String(s))
        | Predicate::Lte(GremlinValue::String(s)) => {
            let right = if let Some(id) = alias_map.get(s.as_str()) {
                Expr::Variable(id.clone())
            } else {
                gremlin_value_to_expr(&GremlinValue::String(s.clone()))
            };
            let op = predicate_to_binary_op(pred);
            Expr::BinaryOp {
                op,
                left: Box::new(left.clone()),
                right: Box::new(right),
            }
        }
        // within("key") where "key" is an aggregate side-effect: left IN __gremlin_agg_key
        Predicate::Within(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                if let Some(id) = alias_map.get(s.as_str()) {
                    return Expr::BinaryOp {
                        op: BinaryOp::In,
                        left: Box::new(left.clone()),
                        right: Box::new(Expr::Variable(id.clone())),
                    };
                }
                if aggregate_keys.contains(s.as_str()) {
                    return Expr::BinaryOp {
                        op: BinaryOp::In,
                        left: Box::new(left.clone()),
                        right: Box::new(Expr::Variable(Identifier::new(format!(
                            "__gremlin_agg_{s}"
                        )))),
                    };
                }
            }
            predicate_to_prop_expr(pred, left.clone())
        }
        // without("key") where "key" is an aggregate side-effect: NOT (left IN __gremlin_agg_key)
        Predicate::Without(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                if let Some(id) = alias_map.get(s.as_str()) {
                    return Expr::UnaryOp {
                        op: cypher_core::ast::UnaryOp::Not,
                        expr: Box::new(Expr::BinaryOp {
                            op: BinaryOp::In,
                            left: Box::new(left.clone()),
                            right: Box::new(Expr::Variable(id.clone())),
                        }),
                    };
                }
                if aggregate_keys.contains(s.as_str()) {
                    return Expr::UnaryOp {
                        op: cypher_core::ast::UnaryOp::Not,
                        expr: Box::new(Expr::BinaryOp {
                            op: BinaryOp::In,
                            left: Box::new(left.clone()),
                            right: Box::new(Expr::Variable(Identifier::new(format!(
                                "__gremlin_agg_{s}"
                            )))),
                        }),
                    };
                }
            }
            predicate_to_prop_expr(pred, left.clone())
        }
        Predicate::Or(preds) => {
            let mut iter = preds.iter();
            let first = predicate_to_label_aware_expr(
                iter.next().unwrap(),
                left,
                alias_map,
                aggregate_keys,
            );
            iter.fold(first, |acc, p| Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(acc),
                right: Box::new(predicate_to_label_aware_expr(
                    p,
                    left,
                    alias_map,
                    aggregate_keys,
                )),
            })
        }
        Predicate::And(preds) => {
            let mut iter = preds.iter();
            let first = predicate_to_label_aware_expr(
                iter.next().unwrap(),
                left,
                alias_map,
                aggregate_keys,
            );
            iter.fold(first, |acc, p| Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(acc),
                right: Box::new(predicate_to_label_aware_expr(
                    p,
                    left,
                    alias_map,
                    aggregate_keys,
                )),
            })
        }
        Predicate::Not(inner) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(predicate_to_label_aware_expr(
                inner,
                left,
                alias_map,
                aggregate_keys,
            )),
        },
        _ => predicate_to_prop_expr(pred, left.clone()),
    }
}

/// in the alias map or aggregate keys. Returns the variable expression for that label if so.
pub(super) fn predicate_label_ref(
    pred: &Predicate,
    alias_map: &HashMap<String, Identifier>,
    aggregate_keys: &HashSet<String>,
) -> Option<Expr> {
    let label = match pred {
        Predicate::Eq(GremlinValue::String(s))
        | Predicate::Neq(GremlinValue::String(s))
        | Predicate::Gt(GremlinValue::String(s))
        | Predicate::Gte(GremlinValue::String(s))
        | Predicate::Lt(GremlinValue::String(s))
        | Predicate::Lte(GremlinValue::String(s)) => s,
        // within("a") / without("a") with a single string argument:
        // if it resolves to a known alias/side-effect, treat it as a
        // variable reference (membership test against the stored list).
        Predicate::Within(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                s
            } else {
                return None;
            }
        }
        Predicate::Without(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                s
            } else {
                return None;
            }
        }
        _ => return None,
    };
    // Check alias_map first (as-labels, side-effects)
    if let Some(id) = alias_map.get(label) {
        return Some(Expr::Variable(id.clone()));
    }
    // Check aggregate keys — resolve to __gremlin_agg_<key>
    if aggregate_keys.contains(label) {
        return Some(Expr::Variable(Identifier::new(format!(
            "__gremlin_agg_{label}"
        ))));
    }
    None
}

/// Extract the BinaryOp from a predicate (ignoring its value).
pub(super) fn predicate_to_binary_op(pred: &Predicate) -> BinaryOp {
    match pred {
        Predicate::Eq(_) => BinaryOp::Eq,
        Predicate::Neq(_) => BinaryOp::Neq,
        Predicate::Gt(_) => BinaryOp::Gt,
        Predicate::Gte(_) => BinaryOp::Gte,
        Predicate::Lt(_) => BinaryOp::Lt,
        Predicate::Lte(_) => BinaryOp::Lte,
        Predicate::Within(_) => BinaryOp::In,
        Predicate::Without(_) => BinaryOp::In,
        Predicate::Between(_, _) => BinaryOp::And,
        Predicate::Inside(_, _) => BinaryOp::And,
        // Text predicates don't map to a single BinaryOp; default to Eq
        Predicate::Containing(_)
        | Predicate::NotContaining(_)
        | Predicate::StartingWith(_)
        | Predicate::NotStartingWith(_)
        | Predicate::EndingWith(_)
        | Predicate::NotEndingWith(_)
        | Predicate::Regex(_)
        | Predicate::NotRegex(_)
        | Predicate::IsType(_)
        | Predicate::And(_)
        | Predicate::Or(_)
        | Predicate::Not(_) => BinaryOp::Eq,
    }
}

#[derive(Clone)]
pub(super) enum WhereByTarget {
    Identifier(Identifier),
    MapEntryKey(Identifier),
    MapEntryValue(Identifier),
}

impl WhereByTarget {
    fn matches_variable(&self, variable: &Identifier) -> bool {
        match self {
            WhereByTarget::Identifier(id)
            | WhereByTarget::MapEntryKey(id)
            | WhereByTarget::MapEntryValue(id) => id == variable,
        }
    }

    fn to_expr(&self) -> Expr {
        match self {
            WhereByTarget::Identifier(id) => Expr::Variable(id.clone()),
            WhereByTarget::MapEntryKey(id) => Expr::FunctionCall {
                name: SmolStr::new("gremlin_unfold_map_entry_key"),
                args: vec![Expr::Variable(id.clone())],
                distinct: false,
            },
            WhereByTarget::MapEntryValue(id) => Expr::FunctionCall {
                name: SmolStr::new("gremlin_unfold_map_entry_value"),
                args: vec![Expr::Variable(id.clone())],
                distinct: false,
            },
        }
    }
}

pub(super) fn extract_where_by_targets(
    steps: &[GremlinStep],
    alias_map: &HashMap<String, Identifier>,
    current_alias: &Identifier,
) -> Option<Vec<WhereByTarget>> {
    // Pattern 1: Select(["a"]) + Is(pred("b")) → where("a", P.gt("b"))
    if steps.len() == 2 {
        if let (GremlinStep::Select(_, keys), GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            if keys.len() == 1 {
                let left = if keys[0] == "keys" || keys[0] == "Column.keys" {
                    WhereByTarget::MapEntryKey(current_alias.clone())
                } else if keys[0] == "values" || keys[0] == "Column.values" {
                    WhereByTarget::MapEntryValue(current_alias.clone())
                } else {
                    WhereByTarget::Identifier(
                        alias_map
                            .get(&keys[0])
                            .cloned()
                            .unwrap_or_else(|| Identifier::new(keys[0].clone())),
                    )
                };
                let mut targets = vec![left];
                collect_predicate_labels_in_order(pred, alias_map, &mut targets);
                if targets.len() > 1 {
                    return Some(targets);
                }
            }
        }
    }
    // Pattern 2: Is(pred("a")) → where(P.gt("a")) compares current vs label
    if steps.len() == 1 {
        if let GremlinStep::Is(pred) = &steps[0] {
            let mut targets = vec![WhereByTarget::Identifier(current_alias.clone())];
            collect_predicate_labels_in_order(pred, alias_map, &mut targets);
            if targets.len() > 1 {
                return Some(targets);
            }
        }
    }
    None
}

pub(super) fn collect_predicate_labels_in_order(
    pred: &Predicate,
    alias_map: &HashMap<String, Identifier>,
    targets: &mut Vec<WhereByTarget>,
) {
    match pred {
        Predicate::Eq(GremlinValue::String(s))
        | Predicate::Neq(GremlinValue::String(s))
        | Predicate::Gt(GremlinValue::String(s))
        | Predicate::Gte(GremlinValue::String(s))
        | Predicate::Lt(GremlinValue::String(s))
        | Predicate::Lte(GremlinValue::String(s)) => {
            if let Some(id) = alias_map.get(s.as_str()) {
                if !targets
                    .iter()
                    .any(|existing| matches!(existing, WhereByTarget::Identifier(existing_id) if existing_id == id))
                {
                    targets.push(WhereByTarget::Identifier(id.clone()));
                }
            }
        }
        Predicate::Within(vals) | Predicate::Without(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                if let Some(id) = alias_map.get(s.as_str()) {
                    if !targets
                        .iter()
                        .any(|existing| matches!(existing, WhereByTarget::Identifier(existing_id) if existing_id == id))
                    {
                        targets.push(WhereByTarget::Identifier(id.clone()));
                    }
                }
            }
        }
        Predicate::Or(preds) | Predicate::And(preds) => {
            for inner in preds {
                collect_predicate_labels_in_order(inner, alias_map, targets);
            }
        }
        Predicate::Not(inner) => collect_predicate_labels_in_order(inner, alias_map, targets),
        _ => {}
    }
}

pub(super) fn finalize_where_by_modulation(
    predicate: &mut Expr,
    targets: &[WhereByTarget],
    modulators: &[String],
    productive_by: bool,
) {
    if targets.is_empty() || modulators.is_empty() {
        return;
    }

    for (index, target_id) in targets.iter().enumerate() {
        let key = &modulators[index % modulators.len()];
        rewrite_where_by_target(predicate, target_id, key);
    }

    if productive_by {
        wrap_where_by_null_eq(predicate);
    }
}

pub(super) fn rewrite_where_by_target(predicate: &mut Expr, target: &WhereByTarget, key: &str) {
    match predicate {
        Expr::BinaryOp { left, right, .. } => {
            rewrite_where_by_expr(left, target, key);
            rewrite_where_by_expr(right, target, key);
        }
        Expr::UnaryOp { expr, .. } => rewrite_where_by_expr(expr, target, key),
        Expr::IsNull(expr) => rewrite_where_by_expr(expr, target, key),
        _ => {}
    }
}

pub(super) fn rewrite_where_by_expr(expr: &mut Expr, target: &WhereByTarget, key: &str) {
    match expr {
        Expr::Variable(v) if target.matches_variable(v) => {
            let base = target.to_expr();
            *expr = if key.is_empty() {
                base
            } else {
                Expr::Property(Box::new(base), SmolStr::new(key))
            };
        }
        Expr::Property(inner, _) => {
            if matches!(inner.as_ref(), Expr::Variable(v) if target.matches_variable(v)) {
                let base = target.to_expr();
                *expr = if key.is_empty() {
                    base
                } else {
                    Expr::Property(Box::new(base), SmolStr::new(key))
                };
            } else {
                rewrite_where_by_expr(inner, target, key);
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            rewrite_where_by_expr(left, target, key);
            rewrite_where_by_expr(right, target, key);
        }
        Expr::UnaryOp { expr: inner, .. } | Expr::IsNull(inner) => {
            rewrite_where_by_expr(inner, target, key);
        }
        _ => {}
    }
}

/// ProductiveByStrategy: for each Eq comparison leaf in a where-by predicate,
/// add `OR (left IS NULL AND right IS NULL)` so that null == null passes.
pub(super) fn wrap_where_by_null_eq(predicate: &mut Expr) {
    if let Expr::BinaryOp { op, left, right } = predicate {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            wrap_where_by_null_eq(left);
            wrap_where_by_null_eq(right);
        } else if matches!(op, BinaryOp::Eq) {
            // Transform: (L == R) → (L == R) OR (L IS NULL AND R IS NULL)
            let left_clone = left.clone();
            let right_clone = right.clone();
            let null_check = Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(Expr::IsNull(left_clone)),
                right: Box::new(Expr::IsNull(right_clone)),
            };
            let original = std::mem::replace(predicate, Expr::Bool(true));
            *predicate = Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(original),
                right: Box::new(null_check),
            };
        }
    }
}

/// Extract a property key string from a simple `by(traversal)` pattern.
///
/// Recognizes patterns like:
///   `[Values(["name"])]`               → `"name"`
///   `[Label]`                           → `"~label"`
///   `[Id]`                              → `"~id"`
///   `[Select(_, ["src","tgt"])]`       → `"~select_map:src,tgt"`
///   `[Values(["name"]), ToUpper]`       → `"~toupper:name"`
///   `[Values(["name"]), ToLower]`       → `"~tolower:name"`
///   `[Unfold, Values(["name"]), Fold]`  → `"~unfold_values_fold:name"`
///   `[In(["created"]), Values(["age"]), Min]` → `"~in_values_min:created:age"`
///
/// Returns empty string for unrecognized patterns (identity behavior).
pub(super) fn extract_by_traversal_key(steps: &[GremlinStep]) -> String {
    match steps {
        [GremlinStep::Values(keys)] if keys.len() == 1 => keys[0].clone(),
        [GremlinStep::Label] => "~label".to_string(),
        [GremlinStep::Id] => "~id".to_string(),
        [GremlinStep::Select(_, keys)] if keys.len() > 1 => {
            format!("~select_map:{}", keys.join(","))
        }
        [GremlinStep::LimitLocal(n)] => format!("~limit_local:{n}"),
        [GremlinStep::TailLocal(n)] => format!("~tail_local:{n}"),
        [GremlinStep::Math(expr)] => format!("~math:{expr}"),
        [GremlinStep::Values(keys), GremlinStep::ToUpper] if keys.len() == 1 => {
            format!("~toupper:{}", keys[0])
        }
        [GremlinStep::Values(keys), GremlinStep::ToLower] if keys.len() == 1 => {
            format!("~tolower:{}", keys[0])
        }
        [GremlinStep::Unfold, GremlinStep::Values(keys), GremlinStep::Fold] if keys.len() == 1 => {
            format!("~unfold_values_fold:{}", keys[0])
        }
        [GremlinStep::In(labels), GremlinStep::Values(keys), GremlinStep::Min]
            if labels.len() == 1 && keys.len() == 1 =>
        {
            format!("~in_values_min:{}:{}", labels[0], keys[0])
        }
        // tail(local, N).values("prop") — for a single element, tail is identity
        [GremlinStep::TailLocal(_), GremlinStep::Values(keys)] if keys.len() == 1 => {
            keys[0].clone()
        }
        _ => String::new(),
    }
}

pub(super) fn extract_select_traversal_label(key: &str) -> Option<String> {
    let inner = key
        .strip_prefix("__.select(")
        .or_else(|| key.strip_prefix("select("))?;
    let body = inner.strip_suffix(')')?.trim();
    if body.contains(',') {
        return None;
    }
    let label = body.trim_matches(|c| c == '\'' || c == '"');
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}

/// Extract variable names from a math expression.
/// Variables are non-numeric, non-"_" tokens that appear as operands in binary expressions.
pub(super) fn extract_math_variables(expr: &str) -> Vec<String> {
    let mut vars = Vec::new();
    let expr = expr.trim();
    // Check tokens around binary operators
    for op in &[" + ", " - ", " * ", " / ", " % "] {
        if let Some(pos) = expr.find(op) {
            let left = expr[..pos].trim();
            let right = expr[pos + op.len()..].trim();
            for token in &[left, right] {
                if *token != "_"
                    && !token.is_empty()
                    && token.parse::<f64>().is_err()
                    && !token.starts_with("ceil")
                    && !token.starts_with("floor")
                    && !token.starts_with("abs")
                    && !token.starts_with("sqrt")
                    && !vars.contains(&token.to_string())
                {
                    vars.push(token.to_string());
                }
            }
        }
    }
    vars
}
