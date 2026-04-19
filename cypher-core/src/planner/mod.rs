//! Query planner — operator-type definitions and the QueryPlanner implementation.
//!
//! This module is a thin re-export hub. The actual definitions live in sibling
//! files:
//!
//! - [`operators`] — physical plan operator enum and supporting types
//! - [`traversal`] — `Operator::children` and visible-alias collection
//! - [`cardinality`] — `GraphStatistics` and cardinality estimation
//! - [`planner`] — the `QueryPlanner` (AST → operator tree)
//! - [`validator`] — query semantic validation

use crate::config;
use crate::error::GqlError;

pub mod cardinality;
pub mod operators;
#[allow(clippy::module_inception)]
pub mod planner;
pub mod traversal;
pub mod validator;

#[cfg(test)]
mod tests;

// Public re-exports preserving the original module surface.
pub use cardinality::{collect_graph_statistics, estimate_cardinality, GraphStatistics};
pub use operators::{
    AggFunction, CreateEdgeSpec, CreateNodeSpec, CreatePattern, GremlinPop, GremlinSelectTraversal,
    Operator, RemoveItem, SetItem,
};
pub use planner::QueryPlanner;
pub use traversal::collect_visible_aliases;
pub use validator::validate_query;

/// Returns `true` if `name` is a Cypher aggregation function.
/// Single authoritative implementation used by both planner and validator.
pub(crate) fn is_aggregate_function(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "count"
            | "sum"
            | "avg"
            | "min"
            | "max"
            | "collect"
            | "stdev"
            | "stdevp"
            | "percentilecont"
            | "percentiledisc"
    )
}

/// Validate and normalize variable-length expand bounds.
/// Returns `(effective_min, effective_max)`, capping unbounded `max` at the
/// configured `varlen_max_depth` (env: `GQL_VARLEN_MAX_DEPTH`, default 30).
///
/// When `min > effective_max` (e.g. `*2..1` or `*1..0`), openCypher specifies that the
/// pattern matches zero paths.  We return the bounds as-is; the VarLenExpand executor
/// naturally produces no rows because no depth `d` can satisfy `d >= min && d <= max`
/// when `min > max`.
pub fn validate_varlen_bounds(min: u32, max: Option<u32>) -> Result<(u32, u32), GqlError> {
    let effective_max = max.unwrap_or(config::global().varlen_max_depth);
    Ok((min, effective_max))
}
