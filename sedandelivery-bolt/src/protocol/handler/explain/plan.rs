//! EXPLAIN plan-tree formatting (estimated rows only).

use crate::packstream::value::BoltValue;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packstream::value::BoltMapExt;
    use cypher_core::ast::Identifier;
    use cypher_core::planner::Operator;

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
}
