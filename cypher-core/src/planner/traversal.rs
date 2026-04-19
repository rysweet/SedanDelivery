//! Operator-tree traversal helpers.
//!
//! Provides `Operator::children` for structural traversal and
//! `collect_visible_aliases` for scope analysis.

use super::operators::{GremlinSelectTraversal, Operator};
use crate::ast::Expr;

impl Operator {
    /// Return references to all direct child operators.
    pub fn children(&self) -> Vec<&Operator> {
        match self {
            Operator::SingleRow
            | Operator::NodeScan { .. }
            | Operator::IndexScan { .. }
            | Operator::NodeByIdSeek { .. }
            | Operator::ShortestPath { .. } => vec![],

            Operator::Filter { child, .. }
            | Operator::Project { child, .. }
            | Operator::Aggregate { child, .. }
            | Operator::Distinct { child, .. }
            | Operator::Sort { child, .. }
            | Operator::Limit { child, .. }
            | Operator::Skip { child, .. }
            | Operator::Unwind { child, .. }
            | Operator::Set { child, .. }
            | Operator::Remove { child, .. }
            | Operator::Delete { child, .. }
            | Operator::ProduceResults { child, .. }
            | Operator::Expand { child, .. }
            | Operator::VarLenExpand { child, .. }
            | Operator::AssemblePath { child, .. }
            | Operator::MergeRelationship { child, .. }
            | Operator::GremlinSideEffect { child, .. }
            | Operator::Cap { child, .. }
            | Operator::GremlinAggregate { child, .. }
            | Operator::GremlinPath { child, .. }
            | Operator::GremlinTree { child, .. }
            | Operator::GremlinSample { child, .. }
            | Operator::GremlinMapSlice { child, .. }
            | Operator::GremlinSimplePath { child, .. }
            | Operator::GremlinCyclicPath { child, .. }
            | Operator::GremlinIndex { child, .. }
            | Operator::GremlinBarrier { child, .. } => vec![child],

            Operator::GremlinSelect {
                child,
                by_traversals,
                ..
            } => {
                let mut v = vec![child.as_ref()];
                for entry in by_traversals {
                    if let Some(GremlinSelectTraversal::Traversal { op, .. }) = entry {
                        v.push(op.as_ref());
                    }
                }
                v
            }

            Operator::GremlinGroup {
                child,
                key_traversal,
                value_traversal,
                ..
            } => {
                let mut v = vec![child.as_ref()];
                if let Some((op, _)) = key_traversal {
                    v.push(op.as_ref());
                }
                if let Some((op, _)) = value_traversal {
                    v.push(op.as_ref());
                }
                v
            }

            Operator::GremlinProject { entries, child, .. } => {
                let mut v = vec![child.as_ref()];
                for (_, op, _, _) in entries {
                    v.push(op);
                }
                v
            }

            Operator::ForEach { body, child, .. } | Operator::Repeat { body, child, .. } => {
                vec![child, body]
            }

            Operator::Apply { outer, inner }
            | Operator::LeftOuterApply {
                preserved: outer,
                optional_pattern: inner,
            } => vec![outer, inner],

            Operator::Union { left, right, .. } => vec![left, right],

            Operator::Create { child, .. }
            | Operator::Merge { child, .. }
            | Operator::Call { child, .. } => match child {
                Some(c) => vec![c],
                None => vec![],
            },

            Operator::Choose {
                predicate,
                if_true,
                if_false,
                child,
                ..
            } => {
                let mut v = vec![child.as_ref(), predicate.as_ref(), if_true.as_ref()];
                if let Some(f) = if_false {
                    v.push(f.as_ref());
                }
                v
            }

            Operator::ChooseMap {
                selector,
                options,
                child,
                ..
            } => {
                let mut v = vec![child.as_ref(), selector.as_ref()];
                for (_, branch, _) in options {
                    v.push(branch.as_ref());
                }
                v
            }

            Operator::Coalesce {
                alternatives,
                child,
            } => {
                let mut v = vec![child.as_ref()];
                v.extend(alternatives.iter());
                v
            }

            Operator::GremlinMatch {
                patterns, child, ..
            } => {
                let mut v = vec![child.as_ref()];
                v.extend(patterns.iter());
                v
            }
        }
    }
}

/// Collect user-visible variable aliases from an operator tree.
///
/// Returns aliases that would be available in the row at the output of this operator.
/// Filters out internal aliases (those starting with `__`).
pub fn collect_visible_aliases(op: &Operator) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for alias in collect_all_aliases(op) {
        if !alias.starts_with("__") && seen.insert(alias.clone()) {
            result.push(alias);
        }
    }
    result
}

fn collect_all_aliases(op: &Operator) -> Vec<String> {
    let mut aliases = Vec::new();
    match op {
        Operator::NodeScan { alias, .. }
        | Operator::IndexScan { alias, .. }
        | Operator::NodeByIdSeek { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::Expand {
            rel_alias,
            dst_alias,
            child,
            ..
        } => {
            aliases.extend(collect_all_aliases(child));
            if let Some(ra) = rel_alias {
                aliases.push(ra.0.clone());
            }
            aliases.push(dst_alias.0.clone());
        }
        Operator::VarLenExpand {
            path_alias,
            dst_alias,
            child,
            ..
        } => {
            aliases.extend(collect_all_aliases(child));
            if let Some(pa) = path_alias {
                aliases.push(pa.0.clone());
            }
            aliases.push(dst_alias.0.clone());
        }
        Operator::AssemblePath {
            path_alias, child, ..
        } => {
            aliases.extend(collect_all_aliases(child));
            aliases.push(path_alias.0.clone());
        }
        Operator::Filter { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Distinct { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::Delete { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::Unwind { alias, child, .. } => {
            aliases.extend(collect_all_aliases(child));
            aliases.push(alias.0.clone());
        }
        Operator::Apply { outer, inner } => {
            aliases.extend(collect_all_aliases(outer));
            aliases.extend(collect_all_aliases(inner));
        }
        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => {
            aliases.extend(collect_all_aliases(preserved));
            aliases.extend(collect_all_aliases(optional_pattern));
        }
        Operator::Create { patterns, child } => {
            if let Some(ch) = child {
                aliases.extend(collect_all_aliases(ch));
            }
            // Extract aliases from CREATE patterns (newly created nodes/edges).
            for pattern in patterns {
                for (alias, _, _) in &pattern.nodes {
                    if let Some(a) = alias {
                        aliases.push(a.0.clone());
                    }
                }
                for (alias, _, _, _, _, _) in &pattern.edges {
                    if let Some(a) = alias {
                        aliases.push(a.0.clone());
                    }
                }
            }
        }
        Operator::Merge {
            child, node_alias, ..
        } => {
            if let Some(ch) = child {
                aliases.extend(collect_all_aliases(ch));
            }
            if let Some(a) = node_alias {
                aliases.push(a.0.clone());
            }
        }
        Operator::MergeRelationship {
            child, rel_alias, ..
        } => {
            aliases.extend(collect_all_aliases(child));
            if let Some(a) = rel_alias {
                aliases.push(a.0.clone());
            }
        }
        Operator::Project { items, .. } => {
            // After a Project (WITH), only the projected items are visible.
            // Child aliases are hidden (WITH introduces a new scope).
            for (expr, alias) in items {
                if let Some(a) = alias {
                    aliases.push(a.0.clone());
                } else {
                    // Derive column name from expression.
                    match expr {
                        Expr::Variable(id) => aliases.push(id.0.clone()),
                        Expr::Property(obj, key) => {
                            if let Expr::Variable(id) = obj.as_ref() {
                                aliases.push(format!("{}.{}", id.0, key));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Operator::Aggregate {
            keys, aggregations, ..
        } => {
            // After Aggregate, only the key and aggregation aliases are visible.
            for key in keys {
                if let Expr::Variable(id) = key {
                    aliases.push(id.0.clone());
                }
            }
            for (_, _, _, _, alias) in aggregations {
                aliases.push(alias.0.clone());
            }
        }
        Operator::Call {
            yield_items, child, ..
        } => {
            if let Some(ch) = child {
                aliases.extend(collect_all_aliases(ch));
            }
            for (name, alias) in yield_items {
                let col_name = alias.as_ref().unwrap_or(name);
                aliases.push(col_name.0.clone());
            }
        }
        Operator::Union { left, right, .. } => {
            aliases.extend(collect_all_aliases(left));
            aliases.extend(collect_all_aliases(right));
        }
        Operator::ShortestPath {
            path_alias,
            src_alias,
            dst_alias,
            ..
        } => {
            aliases.push(path_alias.0.clone());
            aliases.push(src_alias.0.clone());
            aliases.push(dst_alias.0.clone());
        }
        Operator::ForEach { child, .. } => {
            // FOREACH variable is scoped — not visible after the block.
            // Only propagate aliases from the child pipeline.
            aliases.extend(collect_all_aliases(child));
        }
        Operator::Repeat { child, .. }
        | Operator::Choose { child, .. }
        | Operator::ChooseMap { child, .. }
        | Operator::GremlinSideEffect { child, .. }
        | Operator::GremlinAggregate { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::Coalesce { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinPath { .. } => {
            aliases.push("path".to_string());
        }
        Operator::GremlinTree { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::GremlinMatch { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinSelect { keys, .. } => {
            for (_, output_key) in keys {
                aliases.push(output_key.clone());
            }
        }
        Operator::GremlinGroup { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::GremlinSample { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinMapSlice { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinSimplePath { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinCyclicPath { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinIndex { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinProject { entries, .. } => {
            for (key, _, _, _) in entries {
                aliases.push(key.clone());
            }
        }
        Operator::Cap { keys, .. } => {
            for key in keys {
                aliases.push(key.clone());
            }
        }
        Operator::GremlinBarrier { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::SingleRow | Operator::ProduceResults { .. } => {}
    }
    aliases
}
