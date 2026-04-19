//! Operator tree mutation helpers (sack init, scan injection, alias rewriting).
//!
//! Extracted from `helpers.rs` as part of the helpers/ submodule split.
//! All items are `pub(in crate::compiler)` and behave identically to their pre-split
//! definitions.

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;

/// Inject a `__sack` column into the lowest-level scan/single-row operator
/// by wrapping it in an Apply that adds the initialization value.
pub(in crate::compiler) fn inject_sack_init(op: &mut Operator, init_expr: &Expr) {
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
pub(in crate::compiler) fn inject_scan_into_child(op: Operator, scan: Operator) -> Operator {
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
pub(in crate::compiler) fn rewrite_create_alias(
    op: &mut Operator,
    placeholder: &str,
    replacement: &Identifier,
) {
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

/// Rewrite a sack assignment expression, replacing Expr::Variable references
/// that match `target` with `replacement`. Used by `.by(key)` after `sack(op)`.
pub(in crate::compiler) fn rewrite_sack_by(
    expr: &mut Expr,
    target: &Identifier,
    replacement: &Expr,
) {
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
