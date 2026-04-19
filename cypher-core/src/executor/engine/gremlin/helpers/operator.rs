//! Structural inspection of the planner `Operator` tree, used by the
//! Gremlin `repeat()` executor to decide on dedup / barrier strategies,
//! and a small row-bulk-expansion helper used at fast-path boundaries.

use crate::ast::{Direction, Expr, Identifier};
use crate::executor::engine::Row;
use crate::planner::Operator;
use crate::types::Value;

/// Check whether an operator tree contains a `Distinct` node (shallow search).
/// Used by `exec_repeat` to decide whether to enable cross-iteration dedup.
pub(in crate::executor::engine::gremlin) fn operator_contains_distinct(op: &Operator) -> bool {
    match op {
        Operator::Distinct { .. } => true,
        // Walk through wrappers that commonly sit between Repeat body root
        // and the actual Distinct.
        Operator::Project { child, .. }
        | Operator::Filter { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Expand { child, .. }
        | Operator::GremlinSideEffect { child, .. }
        | Operator::Coalesce { child, .. }
        | Operator::GremlinSimplePath { child, .. }
        | Operator::GremlinCyclicPath { child, .. } => operator_contains_distinct(child),
        Operator::Apply { outer, inner } => {
            operator_contains_distinct(outer) || operator_contains_distinct(inner)
        }
        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => operator_contains_distinct(preserved) || operator_contains_distinct(optional_pattern),
        Operator::Union { left, right, .. } => {
            operator_contains_distinct(left) || operator_contains_distinct(right)
        }
        _ => false,
    }
}

/// Returns `true` if the operator tree contains a barrier step (Sort, Aggregate,
/// Distinct) that must see all input rows before producing output. When a repeat
/// body contains barrier steps, batched seed processing must be disabled because
/// each batch would run the barrier independently, producing incorrect results.
pub(in crate::executor::engine::gremlin) fn operator_contains_barrier(op: &Operator) -> bool {
    match op {
        Operator::Sort { .. }
        | Operator::Aggregate { .. }
        | Operator::Distinct { .. }
        | Operator::GremlinBarrier { .. } => true,
        Operator::Project { child, .. }
        | Operator::Filter { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Expand { child, .. }
        | Operator::GremlinSideEffect { child, .. }
        | Operator::Coalesce { child, .. }
        | Operator::GremlinSimplePath { child, .. }
        | Operator::GremlinCyclicPath { child, .. }
        | Operator::GremlinGroup { child, .. }
        | Operator::GremlinAggregate { child, .. } => operator_contains_barrier(child),
        // gremlin_tail limits need all rows → treat as barrier.
        Operator::Limit { count, child } => {
            if matches!(count, Expr::FunctionCall { name, .. } if name.as_str() == "gremlin_tail") {
                true
            } else {
                operator_contains_barrier(child)
            }
        }
        Operator::Apply { outer, inner } => {
            operator_contains_barrier(outer) || operator_contains_barrier(inner)
        }
        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => operator_contains_barrier(preserved) || operator_contains_barrier(optional_pattern),
        Operator::Union { left, right, .. } => {
            operator_contains_barrier(left) || operator_contains_barrier(right)
        }
        Operator::Repeat { body, .. } => operator_contains_barrier(body),
        Operator::Choose {
            if_true, if_false, ..
        } => {
            operator_contains_barrier(if_true)
                || if_false
                    .as_ref()
                    .is_some_and(|f| operator_contains_barrier(f))
        }
        _ => false,
    }
}

/// Extract direction, rel_types, src_alias, dst_alias from a simple Expand
/// whose child is SingleRow.
pub(in crate::executor::engine::gremlin) fn extract_simple_expand(
    op: &Operator,
) -> Option<(Direction, Vec<crate::ast::RelType>, Identifier, Identifier)> {
    match op {
        Operator::Expand {
            direction,
            rel_types,
            src_alias,
            dst_alias,
            child,
            ..
        } if matches!(child.as_ref(), Operator::SingleRow) => Some((
            *direction,
            rel_types.clone(),
            src_alias.clone(),
            dst_alias.clone(),
        )),
        Operator::Project { child, .. } => extract_simple_expand(child),
        _ => None,
    }
}

/// Returns true if the operator tree contains a GremlinSimplePath filter.
pub(in crate::executor::engine::gremlin) fn body_contains_simple_path(op: &Operator) -> bool {
    match op {
        Operator::GremlinSimplePath { .. } => true,
        Operator::Expand { child, .. }
        | Operator::Project { child, .. }
        | Operator::Filter { child, .. }
        | Operator::GremlinCyclicPath { child, .. } => body_contains_simple_path(child),
        _ => false,
    }
}

/// Expand rows that carry `__gremlin_bulk` into individual rows.
///
/// The vertex-counting fast path collapses identical destination nodes into
/// a single row with a bulk count.  Downstream steps (tail, path, select,
/// group-by-count, etc.) expect individual traverser rows, so we expand
/// here at the fast-path boundary.
pub(in crate::executor::engine::gremlin) fn expand_bulk_rows(rows: Vec<Row>) -> Vec<Row> {
    let mut out = Vec::with_capacity(rows.len());
    for mut row in rows {
        if let Some(Value::Int(bulk)) = row.remove("__gremlin_bulk") {
            let n = (bulk as usize).max(1);
            for _ in 0..n {
                out.push(row.clone());
            }
        } else {
            out.push(row);
        }
    }
    out
}
