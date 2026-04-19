//! Cardinality estimation for query plans.
//!
//! Provides `GraphStatistics` and a recursive estimator that maps each
//! operator in a plan tree to its expected output row count.

use std::collections::HashMap;

use super::operators::Operator;
use crate::ast::{Expr, Label};

pub struct GraphStatistics {
    /// Total node count across all labels.
    pub total_nodes: u64,
    /// Total edge count across all relationship types.
    pub total_edges: u64,
    /// Node count per label (label name -> count).
    pub nodes_per_label: HashMap<String, u64>,
    /// Edge count per relationship type (type name -> count).
    pub edges_per_type: HashMap<String, u64>,
}

impl GraphStatistics {
    /// Average out-degree: total_edges / total_nodes (or 1.0 if no nodes).
    fn avg_degree(&self) -> f64 {
        if self.total_nodes == 0 {
            1.0
        } else {
            self.total_edges as f64 / self.total_nodes as f64
        }
    }

    /// Average out-degree for a specific relationship type.
    fn avg_degree_for_type(&self, rel_type: &str) -> f64 {
        if self.total_nodes == 0 {
            return 1.0;
        }
        let edge_count = self
            .edges_per_type
            .get(rel_type)
            .copied()
            .unwrap_or(self.total_edges);
        edge_count as f64 / self.total_nodes as f64
    }
}

/// Default selectivity for unknown filter predicates.
const DEFAULT_FILTER_SELECTIVITY: f64 = 0.1;

/// Estimate cardinality (row count) for each operator in a plan tree.
///
/// Returns a `HashMap<*const Operator, f64>` mapping each operator pointer to its
/// estimated row count. Using raw pointers as keys avoids cloning operators;
/// the map is only valid while the operator tree is alive.
pub fn estimate_cardinality(
    op: &Operator,
    stats: &GraphStatistics,
) -> HashMap<*const Operator, f64> {
    let mut estimates: HashMap<*const Operator, f64> = HashMap::new();
    estimate_recursive(op, stats, &mut estimates);
    estimates
}

fn estimate_recursive(
    op: &Operator,
    stats: &GraphStatistics,
    estimates: &mut HashMap<*const Operator, f64>,
) -> f64 {
    let est = match op {
        Operator::SingleRow => 1.0,

        Operator::NodeScan { label, .. } => match label {
            Some(Label(l)) => stats.nodes_per_label.get(l.as_str()).copied().unwrap_or(0) as f64,
            None => stats.total_nodes as f64,
        },

        Operator::IndexScan { label, .. } => {
            let label_count = stats
                .nodes_per_label
                .get(label.0.as_str())
                .copied()
                .unwrap_or(0) as f64;
            // Index scan with equality: estimate ~10% of label count, at least 1
            (label_count * DEFAULT_FILTER_SELECTIVITY).max(1.0)
        }

        Operator::NodeByIdSeek { .. } => 1.0,

        Operator::Filter { child, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            (child_est * DEFAULT_FILTER_SELECTIVITY).max(1.0)
        }

        Operator::Expand {
            rel_types, child, ..
        } => {
            let child_est = estimate_recursive(child, stats, estimates);
            let degree = if rel_types.len() == 1 {
                stats.avg_degree_for_type(&rel_types[0].0)
            } else {
                stats.avg_degree()
            };
            child_est * degree.max(1.0)
        }

        Operator::VarLenExpand {
            min, max, child, ..
        } => {
            let child_est = estimate_recursive(child, stats, estimates);
            let degree = stats.avg_degree().max(1.0);
            // Estimate: sum of degree^depth for depth in min..=max, simplified
            let depth_range = (*max as f64 - *min as f64 + 1.0).max(1.0);
            child_est * degree * depth_range
        }

        Operator::Limit { count, child, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            match count {
                Expr::Integer(n) => child_est.min(*n as f64),
                _ => child_est.min(100.0), // default limit guess
            }
        }

        Operator::Skip { count, child, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            match count {
                Expr::Integer(n) => (child_est - *n as f64).max(0.0),
                _ => (child_est - 10.0).max(0.0),
            }
        }

        Operator::Project { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::Delete { child, .. }
        | Operator::ProduceResults { child, .. }
        | Operator::AssemblePath { child, .. }
        | Operator::MergeRelationship { child, .. } => estimate_recursive(child, stats, estimates),

        Operator::ForEach { child, body, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            // Body is executed for side effects; estimate it but don't use it for row count.
            let _ = estimate_recursive(body, stats, estimates);
            child_est
        }

        Operator::Distinct { child, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            // Assume distinct removes ~20% of rows
            (child_est * 0.8).max(1.0)
        }

        Operator::Aggregate { child, .. } => {
            let _child_est = estimate_recursive(child, stats, estimates);
            // Aggregation typically reduces to a small number of groups
            10.0_f64.min(stats.total_nodes as f64).max(1.0)
        }

        Operator::Unwind { child, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            child_est * 5.0 // assume average list length of 5
        }

        Operator::Union { left, right, .. } => {
            let l = estimate_recursive(left, stats, estimates);
            let r = estimate_recursive(right, stats, estimates);
            l + r
        }

        Operator::Apply { outer, inner } => {
            let o = estimate_recursive(outer, stats, estimates);
            let i = estimate_recursive(inner, stats, estimates);
            o * i
        }

        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => {
            let p = estimate_recursive(preserved, stats, estimates);
            let o = estimate_recursive(optional_pattern, stats, estimates);
            p * o.max(1.0)
        }

        Operator::Create { child, .. }
        | Operator::Merge { child, .. }
        | Operator::Call { child, .. } => match child {
            Some(c) => estimate_recursive(c, stats, estimates),
            None => 1.0,
        },

        Operator::ShortestPath { .. } => 1.0,

        // Gremlin operators: conservative estimates.
        Operator::Repeat { child, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            child_est * 3.0 // assume ~3 iterations on average
        }
        Operator::Choose { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::ChooseMap { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::Coalesce { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::GremlinSideEffect { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::GremlinAggregate { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::GremlinPath { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::GremlinTree {
            side_effect_key: Some(_),
            child,
            ..
        } => estimate_recursive(child, stats, estimates),
        Operator::GremlinTree { .. } => 1.0, // barrier mode: produces a single Map row
        Operator::GremlinMatch { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::GremlinSelect { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::GremlinGroup {
            side_effect_key: Some(_),
            child,
            ..
        } => estimate_recursive(child, stats, estimates),
        Operator::GremlinGroup { .. } => 1.0, // produces a single Map row
        Operator::GremlinSample { child, .. }
        | Operator::GremlinMapSlice { child, .. }
        | Operator::GremlinSimplePath { child, .. }
        | Operator::GremlinCyclicPath { child, .. }
        | Operator::GremlinIndex { child, .. }
        | Operator::GremlinProject { child, .. }
        | Operator::GremlinBarrier { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::Cap { .. } => 1.0,
    };

    estimates.insert(op as *const Operator, est);
    est
}

/// Build `GraphStatistics` from a `GraphStore` for a given graph.
pub fn collect_graph_statistics(
    store: &dyn crate::storage::GraphStore,
    graph: &str,
) -> Result<GraphStatistics, crate::error::GqlError> {
    let gs = store
        .graph_stats(graph)
        .map_err(crate::error::GqlError::from)?;
    let label_names = store
        .all_label_names(graph)
        .map_err(crate::error::GqlError::from)?;
    let mut nodes_per_label = HashMap::new();
    for label in &label_names {
        let ids = store
            .find_nodes_by_label(graph, label)
            .map_err(crate::error::GqlError::from)?;
        nodes_per_label.insert(label.clone(), ids.len() as u64);
    }
    // Edge counts per type: use find_edges_by_type for each known type.
    // For now, we don't have an `all_rel_type_names` method, so we skip per-type
    // counts and rely on the total edge count + avg degree.
    Ok(GraphStatistics {
        total_nodes: gs.node_count,
        total_edges: gs.edge_count,
        nodes_per_label,
        edges_per_type: HashMap::new(),
    })
}
