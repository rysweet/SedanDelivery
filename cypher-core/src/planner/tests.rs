//! Tests for the planner module (cardinality + varlen bounds).

use std::collections::HashMap;

use super::cardinality::{estimate_cardinality, GraphStatistics};
use super::operators::Operator;
use super::validate_varlen_bounds;
use crate::ast::{Direction, Expr, Identifier, Label};
use crate::config;

#[test]
fn varlen_bounds_valid() {
    assert_eq!(validate_varlen_bounds(1, Some(5)).unwrap(), (1, 5));
    assert_eq!(
        validate_varlen_bounds(0, None).unwrap(),
        (0, config::global().varlen_max_depth)
    );
}

#[test]
fn varlen_bounds_min_gt_max_returns_bounds() {
    // openCypher: min > max is not an error — the pattern simply matches no paths.
    // The VarLenExpand executor naturally returns 0 rows when no depth d satisfies d >= min && d <= max.
    assert_eq!(validate_varlen_bounds(5, Some(3)).unwrap(), (5, 3));
}

// ── Cardinality estimation tests ─────────────────────────────────────────

fn test_stats() -> GraphStatistics {
    let mut nodes_per_label = HashMap::new();
    nodes_per_label.insert("Person".to_string(), 100);
    nodes_per_label.insert("City".to_string(), 10);
    GraphStatistics {
        total_nodes: 110,
        total_edges: 200,
        nodes_per_label,
        edges_per_type: HashMap::new(),
    }
}

#[test]
fn estimate_node_scan_with_label() {
    let stats = test_stats();
    let op = Operator::NodeScan {
        label: Some(Label::new("Person")),
        alias: Identifier::new("n"),
    };
    let estimates = estimate_cardinality(&op, &stats);
    let est = estimates[&(&op as *const Operator)];
    assert!((est - 100.0).abs() < f64::EPSILON);
}

#[test]
fn estimate_node_scan_no_label() {
    let stats = test_stats();
    let op = Operator::NodeScan {
        label: None,
        alias: Identifier::new("n"),
    };
    let estimates = estimate_cardinality(&op, &stats);
    let est = estimates[&(&op as *const Operator)];
    assert!((est - 110.0).abs() < f64::EPSILON);
}

#[test]
fn estimate_filter_applies_selectivity() {
    let stats = test_stats();
    let op = Operator::Filter {
        predicate: Expr::Bool(true),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        }),
    };
    let estimates = estimate_cardinality(&op, &stats);
    let est = estimates[&(&op as *const Operator)];
    // 100 * 0.1 = 10
    assert!((est - 10.0).abs() < f64::EPSILON);
}

#[test]
fn estimate_limit_caps_rows() {
    let stats = test_stats();
    let op = Operator::Limit {
        count: Expr::Integer(5),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        }),
    };
    let estimates = estimate_cardinality(&op, &stats);
    let est = estimates[&(&op as *const Operator)];
    assert!((est - 5.0).abs() < f64::EPSILON);
}

#[test]
fn estimate_expand_multiplies_by_degree() {
    let stats = test_stats();
    let op = Operator::Expand {
        direction: Direction::Outgoing,
        rel_types: vec![],
        src_alias: Identifier::new("n"),
        rel_alias: None,
        dst_alias: Identifier::new("m"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("City")),
            alias: Identifier::new("n"),
        }),
    };
    let estimates = estimate_cardinality(&op, &stats);
    let est = estimates[&(&op as *const Operator)];
    // 10 nodes * (200/110 avg degree) ≈ 18.18
    let expected = 10.0 * (200.0 / 110.0);
    assert!((est - expected).abs() < 0.01);
}

#[test]
fn estimate_single_row() {
    let stats = test_stats();
    let op = Operator::SingleRow;
    let estimates = estimate_cardinality(&op, &stats);
    let est = estimates[&(&op as *const Operator)];
    assert!((est - 1.0).abs() < f64::EPSILON);
}
