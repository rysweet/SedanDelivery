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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Direction, Identifier, RelType};

    fn empty_stats() -> GraphStatistics {
        GraphStatistics {
            total_nodes: 0,
            total_edges: 0,
            nodes_per_label: HashMap::new(),
            edges_per_type: HashMap::new(),
        }
    }

    fn stats_with_nodes(n: u64) -> GraphStatistics {
        let mut s = empty_stats();
        s.total_nodes = n;
        s.total_edges = n * 2;
        s
    }

    fn id(s: &str) -> Identifier {
        Identifier::new(s)
    }

    fn ns(name: &str) -> Operator {
        Operator::NodeScan {
            label: None,
            alias: id(name),
        }
    }

    #[test]
    fn avg_degree_zero_nodes_returns_one() {
        assert_eq!(empty_stats().avg_degree(), 1.0);
    }

    #[test]
    fn avg_degree_normal() {
        let s = stats_with_nodes(10);
        assert_eq!(s.avg_degree(), 2.0);
    }

    #[test]
    fn avg_degree_for_type_uses_edge_count_for_type() {
        let mut s = stats_with_nodes(10);
        s.edges_per_type.insert("FOO".to_string(), 30);
        assert_eq!(s.avg_degree_for_type("FOO"), 3.0);
    }

    #[test]
    fn avg_degree_for_type_zero_nodes() {
        assert_eq!(empty_stats().avg_degree_for_type("X"), 1.0);
    }

    #[test]
    fn avg_degree_for_type_falls_back_to_total() {
        let s = stats_with_nodes(10);
        // No specific entry for "BAR" → falls back to total_edges
        assert_eq!(s.avg_degree_for_type("BAR"), 2.0);
    }

    #[test]
    fn estimate_singlerow() {
        let est = estimate_cardinality(&Operator::SingleRow, &empty_stats());
        assert_eq!(est.values().next().copied(), Some(1.0));
    }

    #[test]
    fn estimate_node_scan_no_label_uses_total() {
        let s = stats_with_nodes(50);
        let op = ns("n");
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(50.0));
    }

    #[test]
    fn estimate_node_scan_with_label() {
        let mut s = empty_stats();
        s.nodes_per_label.insert("P".to_string(), 17);
        let op = Operator::NodeScan {
            label: Some(crate::ast::Label::new("P")),
            alias: id("n"),
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(17.0));
    }

    #[test]
    fn estimate_index_scan_uses_label_count() {
        let mut s = empty_stats();
        s.nodes_per_label.insert("P".to_string(), 100);
        let op = Operator::IndexScan {
            label: crate::ast::Label::new("P"),
            property: "k".to_string(),
            value: None,
            alias: id("n"),
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(10.0));
    }

    #[test]
    fn estimate_node_by_id_seek() {
        let op = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(1),
            alias: id("n"),
        };
        let est = estimate_cardinality(&op, &empty_stats());
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(1.0));
    }

    #[test]
    fn estimate_filter_applies_selectivity() {
        let s = stats_with_nodes(100);
        let op = Operator::Filter {
            predicate: Expr::Bool(true),
            child: Box::new(ns("n")),
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(10.0));
    }

    #[test]
    fn estimate_expand_with_single_rel_type() {
        let s = stats_with_nodes(100);
        let op = Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            src_alias: id("a"),
            rel_alias: None,
            dst_alias: id("b"),
            child: Box::new(ns("a")),
        };
        let est = estimate_cardinality(&op, &s);
        // 100 * avg_degree_for_type=2 = 200
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(200.0));
    }

    #[test]
    fn estimate_expand_multi_rel_uses_avg() {
        let s = stats_with_nodes(50);
        let op = Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R1"), RelType::new("R2")],
            src_alias: id("a"),
            rel_alias: None,
            dst_alias: id("b"),
            child: Box::new(ns("a")),
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(100.0));
    }

    #[test]
    fn estimate_varlen_expand() {
        let s = stats_with_nodes(10);
        let op = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![],
            min: 1,
            max: 3,
            src_alias: id("a"),
            path_alias: None,
            path_mode: false,
            dst_alias: id("b"),
            child: Box::new(ns("a")),
            rel_props: None,
        };
        let est = estimate_cardinality(&op, &s);
        assert!(*est.get(&(&op as *const _)).unwrap() > 0.0);
    }

    #[test]
    fn estimate_limit_caps_child() {
        let s = stats_with_nodes(1000);
        let op = Operator::Limit {
            count: Expr::Integer(5),
            child: Box::new(ns("n")),
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(5.0));
    }

    #[test]
    fn estimate_limit_non_int_uses_default() {
        let s = stats_with_nodes(1000);
        let op = Operator::Limit {
            count: Expr::Variable(id("p")),
            child: Box::new(ns("n")),
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(100.0));
    }

    #[test]
    fn estimate_skip_subtracts() {
        let s = stats_with_nodes(100);
        let op = Operator::Skip {
            count: Expr::Integer(20),
            child: Box::new(ns("n")),
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(80.0));
    }

    #[test]
    fn estimate_skip_non_int_subtracts_default() {
        let s = stats_with_nodes(100);
        let op = Operator::Skip {
            count: Expr::Variable(id("p")),
            child: Box::new(ns("n")),
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(90.0));
    }

    #[test]
    fn estimate_distinct_reduces() {
        let s = stats_with_nodes(100);
        let op = Operator::Distinct {
            child: Box::new(ns("n")),
            by_key: None,
            productive: false,
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(80.0));
    }

    #[test]
    fn estimate_aggregate_collapses() {
        let s = stats_with_nodes(100);
        let op = Operator::Aggregate {
            keys: vec![],
            aggregations: vec![],
            child: Box::new(ns("n")),
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(10.0));
    }

    #[test]
    fn estimate_unwind_multiplies() {
        let s = stats_with_nodes(10);
        let op = Operator::Unwind {
            list_expr: Expr::List(vec![]),
            alias: id("x"),
            child: Box::new(ns("n")),
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(50.0));
    }

    #[test]
    fn estimate_union_sums() {
        let s = stats_with_nodes(10);
        let op = Operator::Union {
            left: Box::new(ns("a")),
            right: Box::new(ns("b")),
            all: true,
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(20.0));
    }

    #[test]
    fn estimate_apply_multiplies() {
        let s = stats_with_nodes(10);
        let op = Operator::Apply {
            outer: Box::new(ns("a")),
            inner: Box::new(ns("b")),
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(100.0));
    }

    #[test]
    fn estimate_left_outer_apply() {
        let s = stats_with_nodes(10);
        let op = Operator::LeftOuterApply {
            preserved: Box::new(ns("a")),
            optional_pattern: Box::new(ns("b")),
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(100.0));
    }

    #[test]
    fn estimate_create_no_child() {
        let op = Operator::Create {
            patterns: vec![],
            child: None,
        };
        let est = estimate_cardinality(&op, &empty_stats());
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(1.0));
    }

    #[test]
    fn estimate_create_with_child() {
        let s = stats_with_nodes(7);
        let op = Operator::Create {
            patterns: vec![],
            child: Some(Box::new(ns("n"))),
        };
        let est = estimate_cardinality(&op, &s);
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(7.0));
    }

    #[test]
    fn estimate_shortest_path_one() {
        let op = Operator::ShortestPath {
            src_alias: id("a"),
            dst_alias: id("b"),
            rel_types: vec![],
            direction: Direction::Outgoing,
            min: 1,
            max: 5,
            path_alias: id("p"),
            all_paths: false,
        };
        let est = estimate_cardinality(&op, &empty_stats());
        assert_eq!(est.get(&(&op as *const _)).copied(), Some(1.0));
    }

    #[test]
    fn estimate_pass_through_operators() {
        let s = stats_with_nodes(7);
        for op in [
            Operator::Project {
                items: vec![],
                child: Box::new(ns("n")),
            },
            Operator::Sort {
                items: vec![],
                gremlin_order: false,
                child: Box::new(ns("n")),
            },
            Operator::Set {
                items: vec![],
                child: Box::new(ns("n")),
            },
            Operator::Remove {
                items: vec![],
                child: Box::new(ns("n")),
            },
            Operator::Delete {
                exprs: vec![],
                detach: false,
                child: Box::new(ns("n")),
            },
            Operator::ProduceResults {
                columns: vec![],
                child: Box::new(ns("n")),
            },
        ] {
            let est = estimate_cardinality(&op, &s);
            assert_eq!(est.get(&(&op as *const _)).copied(), Some(7.0));
        }
    }
}
