//! EXPLAIN plan formatting and cartesian product detection.
//!
//! Extracted from `run.rs` to keep the RUN handler focused on query
//! dispatch while this module owns operator-tree introspection.

use crate::packstream::value::BoltValue;

use super::bolt_map;

// -- Plan conversion ---------------------------------------------------------

/// Convert a cypher-core Operator tree into a Bolt plan map structure.
///
/// Recursively descends into child operators so EXPLAIN returns a proper tree.
/// When `estimates` is provided, populates `EstimatedRows` from the cardinality
/// estimator; otherwise falls back to 1.0.
#[allow(clippy::only_used_in_recursion)]
pub(crate) fn operator_to_plan_map(
    op: &cypher_core::planner::Operator,
    is_profile: bool,
    depth: usize,
    estimates: Option<&std::collections::HashMap<*const cypher_core::planner::Operator, f64>>,
) -> BoltValue {
    use cypher_core::planner::Operator;

    let op_type = format!("{:?}", op)
        .split('(')
        .next()
        .unwrap_or("Unknown")
        .split('{')
        .next()
        .unwrap_or("Unknown")
        .trim()
        .to_string();
    let children_bolt: Vec<BoltValue> = op
        .children()
        .into_iter()
        .map(|child| operator_to_plan_map(child, is_profile, depth + 1, estimates))
        .collect();
    let estimated_rows = estimates
        .and_then(|m| m.get(&(op as *const Operator)))
        .copied()
        .unwrap_or(1.0);
    let entries: Vec<(String, BoltValue)> = vec![
        ("operatorType".to_string(), BoltValue::String(op_type)),
        ("identifiers".to_string(), BoltValue::List(vec![])),
        (
            "args".to_string(),
            BoltValue::Map(vec![
                (
                    "runtime".to_string(),
                    BoltValue::String("INTERPRETED".to_string()),
                ),
                (
                    "EstimatedRows".to_string(),
                    BoltValue::Float(estimated_rows),
                ),
            ]),
        ),
        ("children".to_string(), BoltValue::List(children_bolt)),
    ];
    BoltValue::Map(entries)
}

/// Convert a cypher-core Operator tree into a Bolt plan map structure with
/// per-operator profiling statistics (rows and dbHits).
///
/// Used when PROFILE is active: each operator node includes actual execution
/// metrics in addition to estimated rows.
#[allow(clippy::only_used_in_recursion)]
pub(crate) fn operator_to_profile_plan_map(
    op: &cypher_core::planner::Operator,
    depth: usize,
    estimates: Option<&std::collections::HashMap<*const cypher_core::planner::Operator, f64>>,
    profile_stats: Option<
        &std::collections::HashMap<usize, cypher_core::executor::OperatorProfileStats>,
    >,
) -> BoltValue {
    use cypher_core::planner::Operator;

    let op_type = format!("{:?}", op)
        .split('(')
        .next()
        .unwrap_or("Unknown")
        .split('{')
        .next()
        .unwrap_or("Unknown")
        .trim()
        .to_string();
    let children_bolt: Vec<BoltValue> = op
        .children()
        .into_iter()
        .map(|child| operator_to_profile_plan_map(child, depth + 1, estimates, profile_stats))
        .collect();
    let estimated_rows = estimates
        .and_then(|m| m.get(&(op as *const Operator)))
        .copied()
        .unwrap_or(1.0);

    let op_addr = op as *const Operator as usize;
    let (actual_rows, db_hits) = profile_stats
        .and_then(|stats| stats.get(&op_addr))
        .map(|s| (s.rows as i64, s.db_hits as i64))
        .unwrap_or((0, 0));

    let entries: Vec<(String, BoltValue)> = vec![
        ("operatorType".to_string(), BoltValue::String(op_type)),
        ("identifiers".to_string(), BoltValue::List(vec![])),
        (
            "args".to_string(),
            BoltValue::Map(vec![
                (
                    "runtime".to_string(),
                    BoltValue::String("INTERPRETED".to_string()),
                ),
                (
                    "EstimatedRows".to_string(),
                    BoltValue::Float(estimated_rows),
                ),
            ]),
        ),
        ("children".to_string(), BoltValue::List(children_bolt)),
        ("rows".to_string(), BoltValue::Int(actual_rows)),
        ("dbHits".to_string(), BoltValue::Int(db_hits)),
    ];
    BoltValue::Map(entries)
}

// -- Notification detection --------------------------------------------------

/// Detect cartesian product patterns in an operator tree.
///
/// A cartesian product occurs when an `Apply` node joins two independent scans
/// (no shared variable references). Returns a list of Bolt notification maps.
pub(crate) fn detect_notifications(op: &cypher_core::planner::Operator) -> Vec<BoltValue> {
    let mut notifications = Vec::new();
    detect_cartesian_products(op, &mut notifications);
    notifications
}

/// Walk the operator tree, emitting a notification for each `Apply` whose
/// outer and inner branches share no visible aliases (i.e. a cartesian product).
///
/// Uses `Operator::children()` for recursive descent into non-Apply nodes,
/// avoiding a duplicated match over every variant.
fn detect_cartesian_products(
    op: &cypher_core::planner::Operator,
    notifications: &mut Vec<BoltValue>,
) {
    use cypher_core::planner::Operator;

    if let Operator::Apply { outer, inner } = op {
        // An Apply with two independent scans (both are leaf-level node scans
        // or have no shared aliases) is a cartesian product.
        let outer_aliases: std::collections::HashSet<String> =
            cypher_core::planner::collect_visible_aliases(outer)
                .into_iter()
                .collect();
        let inner_aliases: std::collections::HashSet<String> =
            cypher_core::planner::collect_visible_aliases(inner)
                .into_iter()
                .collect();
        if outer_aliases.is_disjoint(&inner_aliases) {
            // Determine which alias to mention in the warning.
            let inner_alias = inner_aliases
                .into_iter()
                .next()
                .unwrap_or_else(|| "?".to_string());
            notifications.push(BoltValue::Map(bolt_map! {
                "code" => BoltValue::String(
                    "Neo.ClientNotification.Statement.CartesianProductWarning".to_string()
                ),
                "title" => BoltValue::String(
                    "This query builds a cartesian product between disconnected patterns.".to_string()
                ),
                "description" => BoltValue::String(
                    format!(
                        "If a part of a query contains multiple disconnected patterns, this will build a cartesian product between all those parts. This may produce a large amount of data and slow down query processing. While occasionally intended, it may often be possible to reformulate the query that avoids the use of this cross product, perhaps by adding a relationship between the different parts or by using OPTIONAL MATCH (identifier is: ({inner_alias}))"
                    )
                ),
                "severity" => BoltValue::String("WARNING".to_string()),
                "category" => BoltValue::String("PERFORMANCE".to_string()),
                "position" => BoltValue::Map(bolt_map! {
                    "offset" => BoltValue::Int(0),
                    "line" => BoltValue::Int(1),
                    "column" => BoltValue::Int(1)
                })
            }));
        }
    }

    // Recurse into all children using Operator::children() instead of
    // duplicating match arms for every variant.
    for child in op.children() {
        detect_cartesian_products(child, notifications);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packstream::value::BoltMapExt;
    use cypher_core::ast::Identifier;
    use cypher_core::planner::Operator;

    // ── operator_to_plan_map ────────────────────────────────────────────

    #[test]
    fn operator_to_plan_map_single_row() {
        let op = Operator::SingleRow;
        let plan = operator_to_plan_map(&op, false, 0, None);
        match plan {
            BoltValue::Map(entries) => {
                let op_type = entries.get_str("operatorType").unwrap();
                assert_eq!(op_type, "SingleRow");
                assert!(entries.get_value("children").is_some());
                assert!(entries.get_value("args").is_some());
                assert!(entries.get_value("identifiers").is_some());
            }
            _ => panic!("expected BoltValue::Map"),
        }
    }

    #[test]
    fn operator_to_plan_map_node_scan() {
        let op = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let plan = operator_to_plan_map(&op, false, 0, None);
        match plan {
            BoltValue::Map(entries) => {
                let op_type = entries.get_str("operatorType").unwrap();
                assert_eq!(op_type, "NodeScan");
            }
            _ => panic!("expected BoltValue::Map"),
        }
    }

    #[test]
    fn operator_to_plan_map_filter_wrapping_scan() {
        let child = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let op = Operator::Filter {
            predicate: cypher_core::ast::Expr::Bool(true),
            child: Box::new(child),
        };
        let plan = operator_to_plan_map(&op, false, 0, None);
        match plan {
            BoltValue::Map(entries) => {
                let op_type = entries.get_str("operatorType").unwrap();
                assert_eq!(op_type, "Filter");
                if let Some(BoltValue::List(children)) = entries.get_value("children") {
                    assert_eq!(children.len(), 1, "Filter should have one child (NodeScan)");
                    // Verify child is a NodeScan
                    if let BoltValue::Map(child_entries) = &children[0] {
                        let child_type = child_entries.get_str("operatorType").unwrap();
                        assert_eq!(child_type, "NodeScan");
                    } else {
                        panic!("expected child to be a Map");
                    }
                }
            }
            _ => panic!("expected BoltValue::Map"),
        }
    }

    #[test]
    fn operator_to_plan_map_has_runtime_in_args() {
        let op = Operator::SingleRow;
        let plan = operator_to_plan_map(&op, true, 0, None);
        match plan {
            BoltValue::Map(entries) => {
                if let Some(BoltValue::Map(args)) = entries.get_value("args") {
                    let runtime = args.get_str("runtime").unwrap();
                    assert_eq!(runtime, "INTERPRETED");
                } else {
                    panic!("expected args to be a Map");
                }
            }
            _ => panic!("expected BoltValue::Map"),
        }
    }

    // ── detect_notifications / detect_cartesian_products ────────────────

    #[test]
    fn detect_notifications_empty_for_single_scan() {
        let op = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let notes = detect_notifications(&op);
        assert!(notes.is_empty());
    }

    #[test]
    fn detect_cartesian_product_on_disjoint_apply() {
        let outer = Operator::NodeScan {
            label: None,
            alias: Identifier::new("a"),
        };
        let inner = Operator::NodeScan {
            label: None,
            alias: Identifier::new("b"),
        };
        let op = Operator::Apply {
            outer: Box::new(outer),
            inner: Box::new(inner),
        };
        let notes = detect_notifications(&op);
        assert_eq!(notes.len(), 1);
        if let BoltValue::Map(entries) = &notes[0] {
            let code = entries.get_str("code").unwrap();
            assert!(code.contains("CartesianProduct"));
            let severity = entries.get_str("severity").unwrap();
            assert_eq!(severity, "WARNING");
            let category = entries.get_str("category").unwrap();
            assert_eq!(category, "PERFORMANCE");
        } else {
            panic!("expected Map notification");
        }
    }

    #[test]
    fn detect_no_cartesian_product_on_shared_alias() {
        let outer = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let inner = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let op = Operator::Apply {
            outer: Box::new(outer),
            inner: Box::new(inner),
        };
        let notes = detect_notifications(&op);
        assert!(
            notes.is_empty(),
            "shared alias should not trigger cartesian warning"
        );
    }

    #[test]
    fn detect_cartesian_products_recurses_through_filter() {
        let outer = Operator::NodeScan {
            label: None,
            alias: Identifier::new("a"),
        };
        let inner = Operator::NodeScan {
            label: None,
            alias: Identifier::new("b"),
        };
        let apply = Operator::Apply {
            outer: Box::new(outer),
            inner: Box::new(inner),
        };
        let filter = Operator::Filter {
            predicate: cypher_core::ast::Expr::Bool(true),
            child: Box::new(apply),
        };
        let notes = detect_notifications(&filter);
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn detect_cartesian_notification_has_position() {
        let outer = Operator::NodeScan {
            label: None,
            alias: Identifier::new("x"),
        };
        let inner = Operator::NodeScan {
            label: None,
            alias: Identifier::new("y"),
        };
        let op = Operator::Apply {
            outer: Box::new(outer),
            inner: Box::new(inner),
        };
        let notes = detect_notifications(&op);
        if let BoltValue::Map(entries) = &notes[0] {
            if let Some(BoltValue::Map(pos)) = entries.get_value("position") {
                assert_eq!(pos.get_int("offset"), Some(0));
                assert_eq!(pos.get_int("line"), Some(1));
                assert_eq!(pos.get_int("column"), Some(1));
            } else {
                panic!("expected position map");
            }
        }
    }

    #[test]
    fn detect_notifications_on_leaf_operators() {
        let single_row = Operator::SingleRow;
        assert!(detect_notifications(&single_row).is_empty());
    }

    #[test]
    fn detect_cartesian_products_union() {
        let outer = Operator::NodeScan {
            label: None,
            alias: Identifier::new("a"),
        };
        let inner = Operator::NodeScan {
            label: None,
            alias: Identifier::new("b"),
        };
        let apply = Operator::Apply {
            outer: Box::new(outer),
            inner: Box::new(inner),
        };
        let right = Operator::SingleRow;
        let union_op = Operator::Union {
            left: Box::new(apply),
            right: Box::new(right),
            all: true,
        };
        let notes = detect_notifications(&union_op);
        assert_eq!(
            notes.len(),
            1,
            "should find cartesian product in left branch of union"
        );
    }
}
