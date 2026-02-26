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

pub use planner::QueryPlanner;

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
