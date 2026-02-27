//! Query planner — operator-type definitions and the QueryPlanner implementation.
//!
//! This module defines the complete physical plan operator set (28 operators)
//! and the QueryPlanner that translates a parsed Query AST into an Operator tree.

use crate::ast::{Direction, Expr, Identifier, Label, RelType};
use crate::error::{ExecutionError, GqlError};
use crate::executor::budget::DEFAULT_VARLEN_MAX_DEPTH;

// ── Physical Plan Operators ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Operator {
    /// Produces exactly one empty row. Used as a leaf for bare `RETURN` (no MATCH).
    SingleRow,
    NodeScan {
        label: Option<Label>,
        alias: Identifier,
    },
    IndexScan {
        label: Label,
        property: String,
        alias: Identifier,
    },
    NodeByIdSeek {
        id_expr: Expr,
        alias: Identifier,
    },
    Filter {
        predicate: Expr,
        child: Box<Operator>,
    },
    Expand {
        direction: Direction,
        rel_types: Vec<RelType>,
        src_alias: Identifier,
        rel_alias: Option<Identifier>,
        dst_alias: Identifier,
        child: Box<Operator>,
    },
    /// Invariant: min <= max; max <= DEFAULT_VARLEN_MAX_DEPTH when query had no upper bound.
    VarLenExpand {
        direction: Direction,
        rel_types: Vec<RelType>,
        min: u32,
        max: u32,
        src_alias: Identifier,
        path_alias: Option<Identifier>,
        dst_alias: Identifier,
        child: Box<Operator>,
    },
    /// OPTIONAL MATCH: preserves left side rows even when right side matches nothing.
    LeftOuterApply {
        preserved: Box<Operator>,
        optional_pattern: Box<Operator>,
    },
    Project {
        items: Vec<(Expr, Option<Identifier>)>,
        child: Box<Operator>,
    },
    Aggregate {
        keys: Vec<Expr>,
        // (function, argument_expr, distinct, output_alias)
        aggregations: Vec<(AggFunction, Expr, bool, Identifier)>,
        child: Box<Operator>,
    },
    Distinct {
        child: Box<Operator>,
    },
    Sort {
        items: Vec<(Expr, bool)>, // (expr, ascending)
        child: Box<Operator>,
    },
    Limit {
        count: Expr,
        child: Box<Operator>,
    },
    Skip {
        count: Expr,
        child: Box<Operator>,
    },
    Unwind {
        list_expr: Expr,
        alias: Identifier,
        child: Box<Operator>,
    },
    Union {
        left: Box<Operator>,
        right: Box<Operator>,
        all: bool,
    },
    ShortestPath {
        src_alias: Identifier,
        dst_alias: Identifier,
        rel_types: Vec<RelType>,
        direction: Direction,
        min: u32,
        max: u32,
        path_alias: Identifier,
        all_paths: bool,
    },
    Apply {
        outer: Box<Operator>,
        inner: Box<Operator>,
    },
    Create {
        patterns: Vec<CreatePattern>,
        child: Option<Box<Operator>>,
    },
    Merge {
        node_alias: Option<Identifier>,
        labels: Vec<Label>,
        on_create: Vec<SetItem>,
        on_match: Vec<SetItem>,
        child: Option<Box<Operator>>,
    },
    Set {
        items: Vec<SetItem>,
        child: Box<Operator>,
    },
    Remove {
        items: Vec<RemoveItem>,
        child: Box<Operator>,
    },
    Delete {
        exprs: Vec<Expr>,
        detach: bool,
        child: Box<Operator>,
    },
    /// Assembles a named path from node and edge aliases already in the row.
    AssemblePath {
        path_alias: Identifier,
        node_aliases: Vec<Identifier>,
        edge_aliases: Vec<Identifier>,
        child: Box<Operator>,
    },
    /// CALL procedure.
    Call {
        procedure: String,
        args: Vec<Expr>,
        /// YIELD items: `(output_name, optional_alias)`.
        yield_items: Vec<(Identifier, Option<Identifier>)>,
        /// Whether `YIELD *` was used.
        yield_star: bool,
        /// Whether this is a standalone CALL (no subsequent RETURN).
        standalone: bool,
        child: Option<Box<Operator>>,
    },
    /// Root of every plan tree. Explicit result boundary.
    ProduceResults {
        columns: Vec<(String, Expr)>,
        child: Box<Operator>,
    },
}

// ── Supporting types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggFunction {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Collect,
    StDev,
    StDevP,
    PercentileCont,
    PercentileDisc,
}

/// Type alias for a node specification in a CREATE pattern:
/// `(alias, labels, property_pairs)`
pub type CreateNodeSpec = (Option<Identifier>, Vec<Label>, Vec<(String, Expr)>);

/// Type alias for an edge specification in a CREATE pattern:
/// `(alias, src_alias, dst_alias, rel_type, direction, property_pairs)`
pub type CreateEdgeSpec = (
    Option<Identifier>,
    Identifier,
    Identifier,
    RelType,
    Direction,
    Vec<(String, Expr)>,
);

#[derive(Debug, Clone)]
pub struct CreatePattern {
    pub nodes: Vec<CreateNodeSpec>,
    pub edges: Vec<CreateEdgeSpec>,
}

#[derive(Debug, Clone)]
pub enum SetItem {
    Property {
        alias: Identifier,
        key: String,
        value: Expr,
    },
    Labels {
        alias: Identifier,
        labels: Vec<Label>,
    },
    Map {
        alias: Identifier,
        map: Expr,
        merge: bool,
    },
}

#[derive(Debug, Clone)]
pub enum RemoveItem {
    Property {
        alias: Identifier,
        key: String,
    },
    Labels {
        alias: Identifier,
        labels: Vec<Label>,
    },
}

// ── Planner module ────────────────────────────────────────────────────────────

#[allow(clippy::module_inception)]
pub mod planner;
pub mod validator;

pub use planner::QueryPlanner;
pub use validator::validate_query;

/// Validate and normalize variable-length expand bounds.
/// Returns `(effective_min, effective_max)`, capping unbounded `max` at DEFAULT_VARLEN_MAX_DEPTH.
pub fn validate_varlen_bounds(min: u32, max: Option<u32>) -> Result<(u32, u32), GqlError> {
    let effective_max = max.unwrap_or(DEFAULT_VARLEN_MAX_DEPTH);
    if min > effective_max {
        return Err(GqlError::Execution(ExecutionError::unsupported(format!(
            "variable-length pattern: min ({min}) must not exceed max ({effective_max})"
        ))));
    }
    Ok((min, effective_max))
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
            for (_, _, _, alias) in aggregations {
                aliases.push(alias.0.clone());
            }
        }
        Operator::Call {
            yield_items,
            child,
            ..
        } => {
            if let Some(ch) = child {
                aliases.extend(collect_all_aliases(ch));
            }
            for (name, alias) in yield_items {
                let col_name = alias.as_ref().unwrap_or(name);
                aliases.push(col_name.0.clone());
            }
        }
        _ => {}
    }
    aliases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varlen_bounds_valid() {
        assert_eq!(validate_varlen_bounds(1, Some(5)).unwrap(), (1, 5));
        assert_eq!(
            validate_varlen_bounds(0, None).unwrap(),
            (0, DEFAULT_VARLEN_MAX_DEPTH)
        );
    }

    #[test]
    fn varlen_bounds_min_gt_max_is_error() {
        assert!(validate_varlen_bounds(5, Some(3)).is_err());
    }
}
