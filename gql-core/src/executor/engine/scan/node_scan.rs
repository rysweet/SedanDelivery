//! Node scanning operators: NodeScan, IndexScan, NodeByIdSeek.

use std::collections::HashMap;

use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::eval_expr;
use crate::executor::filter::Row as FilterRow;
use crate::planner::Operator;
use crate::storage::{NodeId, RangeQuery};
use crate::types::Value;

use crate::executor::engine::helpers::inject_parameters;
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_node_scan(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::NodeScan { label, alias } = op else {
            debug_assert!(false, "expected NodeScan operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected NodeScan operator".into(),
            }));
        };
        let node_ids = if let Some(lbl) = label {
            self.store.find_nodes_by_label(cx.graph, lbl.as_str())?
        } else {
            self.store.all_node_ids(cx.graph)?
        };

        // When used as the inner side of Apply/LeftOuterApply,
        // outer_rows carries context variables that inner operators
        // (e.g., Filter) may reference. Merge them into each produced row.
        // Exclude internal anonymous edge aliases (__anon_rel_*) — these are
        // per-pattern tracking variables and must not leak into inner patterns.
        let base_rows: &[Row] = if cx.outer_rows.is_empty() {
            &[]
        } else {
            cx.outer_rows
        };

        let mut rows = Vec::with_capacity(node_ids.len());
        for id in &node_ids {
            if cx.budget.reached_row_cap() {
                break;
            }
            cx.budget.check_or_err()?;
            if base_rows.is_empty() {
                let mut row = Row::new();
                row.insert(alias.0.clone(), Value::Node(*id));
                inject_parameters(&mut row, &self.parameters);
                cx.budget.record_row();
                cx.budget.record_leaf_row();
                rows.push(row);
            } else {
                for outer_row in base_rows {
                    if cx.budget.reached_row_cap() {
                        break;
                    }
                    cx.budget.check_or_err()?;
                    // If the alias is already pre-bound in outer_row to a specific node,
                    // only produce a row when the scanned node matches that bound node.
                    // This correctly handles correlated subqueries and repeated MATCH clauses.
                    // Only apply for non-anonymous variables (anonymous vars like __anon*
                    // should always scan all nodes independently).
                    if !alias.0.starts_with("__anon") {
                        if let Some(Value::Node(pre_bound_id)) = outer_row.get(alias.0.as_str()) {
                            if *pre_bound_id != *id {
                                continue; // Skip: doesn't match the pre-bound value
                            }
                        }
                    }
                    let mut row: Row = outer_row
                        .iter()
                        .filter(|(k, _)| !k.starts_with("__anon_rel_"))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    row.insert(alias.0.clone(), Value::Node(*id));
                    // Track outer-scope edge variable names so Expand's isomorphism
                    // check doesn't block edges from previous MATCH clauses.
                    let outer_edge_keys: Vec<Value> = outer_row
                        .iter()
                        .filter(|(k, v)| {
                            !k.starts_with("__anon_rel_")
                                && !k.starts_with("__outer_edge_vars__")
                                && matches!(v, Value::Edge(_))
                        })
                        .map(|(k, _)| Value::Str(k.clone().into_boxed_str()))
                        .collect();
                    if !outer_edge_keys.is_empty() {
                        let mut existing: Vec<Value> = match row.get("__outer_edge_vars__") {
                            Some(Value::List(vs)) => vs.clone(),
                            _ => vec![],
                        };
                        existing.extend(outer_edge_keys);
                        row.insert("__outer_edge_vars__".to_string(), Value::List(existing));
                    }
                    cx.budget.record_row();
                    cx.budget.record_leaf_row();
                    rows.push(row);
                }
            }
        }
        Ok(rows)
    }

    pub(crate) fn exec_index_scan(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::IndexScan {
            label,
            property,
            value,
            alias,
        } = op
        else {
            debug_assert!(false, "expected IndexScan operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected IndexScan operator".into(),
            }));
        };

        let base_rows: &[Row] = if cx.outer_rows.is_empty() {
            &[]
        } else {
            cx.outer_rows
        };

        let resolve_ids = |evaluated_value: &Option<Value>| -> Result<Vec<NodeId>, GqlError> {
            let range_query = match evaluated_value {
                Some(v) => RangeQuery::Eq(v),
                None => RangeQuery::Gte(&Value::Null),
            };
            let node_ids = self.store.find_nodes_by_label_and_property(
                cx.graph,
                label.as_str(),
                property,
                range_query,
            )?;
            if node_ids.is_empty() && evaluated_value.is_none() {
                let all_label = self.store.find_nodes_by_label(cx.graph, label.as_str())?;
                let mut with_prop = Vec::new();
                for id in all_label {
                    if let Some(node) = self.store.get_node(cx.graph, id)? {
                        if node.properties.contains_key(property.as_str()) {
                            with_prop.push(id);
                        }
                    }
                }
                Ok(with_prop)
            } else {
                Ok(node_ids)
            }
        };

        let mut rows = Vec::new();
        if base_rows.is_empty() {
            let evaluated_value: Option<Value> = if let Some(val_expr) = value {
                let empty_row = Row::new();
                let ctx = self.build_filter_ctx(cx.graph, &empty_row)?;
                Some(eval_expr(val_expr, &ctx)?)
            } else {
                None
            };
            let node_ids = resolve_ids(&evaluated_value)?;

            for id in node_ids {
                if cx.budget.reached_row_cap() {
                    break;
                }
                cx.budget.check_or_err()?;
                let mut row = Row::new();
                row.insert(alias.0.clone(), Value::Node(id));
                inject_parameters(&mut row, &self.parameters);
                cx.budget.record_row();
                cx.budget.record_leaf_row();
                rows.push(row);
            }
            return Ok(rows);
        }

        for outer_row in base_rows {
            if cx.budget.reached_row_cap() {
                break;
            }
            cx.budget.check_or_err()?;

            let ctx = self.build_filter_ctx(cx.graph, outer_row)?;
            let evaluated_value: Option<Value> = if let Some(val_expr) = value {
                Some(self.eval_with_storage(cx.graph, val_expr, outer_row, &ctx)?)
            } else {
                None
            };
            let node_ids = resolve_ids(&evaluated_value)?;

            for id in node_ids {
                if cx.budget.reached_row_cap() {
                    break;
                }
                cx.budget.check_or_err()?;
                if !alias.0.starts_with("__anon") {
                    if let Some(Value::Node(pre_bound_id)) = outer_row.get(alias.0.as_str()) {
                        if *pre_bound_id != id {
                            continue;
                        }
                    }
                }
                let mut row: Row = outer_row
                    .iter()
                    .filter(|(k, _)| !k.starts_with("__anon_rel_"))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                row.insert(alias.0.clone(), Value::Node(id));
                inject_parameters(&mut row, &self.parameters);
                let outer_edge_keys: Vec<Value> = outer_row
                    .iter()
                    .filter(|(k, v)| {
                        !k.starts_with("__anon_rel_")
                            && !k.starts_with("__outer_edge_vars__")
                            && matches!(v, Value::Edge(_))
                    })
                    .map(|(k, _)| Value::Str(k.clone().into_boxed_str()))
                    .collect();
                if !outer_edge_keys.is_empty() {
                    let mut existing: Vec<Value> = match row.get("__outer_edge_vars__") {
                        Some(Value::List(vs)) => vs.clone(),
                        _ => vec![],
                    };
                    existing.extend(outer_edge_keys);
                    row.insert("__outer_edge_vars__".to_string(), Value::List(existing));
                }
                cx.budget.record_row();
                cx.budget.record_leaf_row();
                rows.push(row);
            }
            if cx.budget.reached_row_cap() {
                break;
            }
        }
        Ok(rows)
    }

    pub(crate) fn exec_node_by_id_seek(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::NodeByIdSeek { id_expr, alias } = op else {
            debug_assert!(false, "expected NodeByIdSeek operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected NodeByIdSeek operator".into(),
            }));
        };
        let ctx: FilterRow<'_> = HashMap::new();
        let id_val = eval_expr(id_expr, &ctx)?;
        let raw_id = match id_val {
            Value::Int(n) if n >= 0 => n as u64,
            Value::Int(n) => {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "non-negative integer for node ID".to_string(),
                    actual: n.to_string(),
                }));
            }
            other => {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "integer for node ID".to_string(),
                    actual: other.type_name().to_string(),
                }));
            }
        };

        let node_id = NodeId(raw_id);
        let mut rows = Vec::with_capacity(1);
        if let Some(_node_data) = self.store.get_node(cx.graph, node_id)? {
            let mut row = Row::new();
            row.insert(alias.0.clone(), Value::Node(node_id));
            cx.budget.record_row();
            rows.push(row);
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, Identifier, Label};
    use crate::executor::budget::ExecutionBudget;
    use crate::executor::engine::MutationStats;
    use crate::planner::Operator;
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use indexmap::IndexMap;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn node_scan_all_nodes() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_node("g", vec!["B".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn node_scan_with_label_filter() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_node("g", vec!["Movie".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn node_scan_empty_graph() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn node_scan_nonexistent_label() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: Some(Label::new("NonExistent")),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn node_scan_binds_correct_alias() {
        let store = make_store();
        let nid = store
            .create_node("g", vec!["P".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("mynode"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].get("mynode"), Some(&Value::Node(nid)));
    }

    #[test]
    fn node_scan_budget_enforced_in_inner_loop() {
        use std::time::Duration;

        let store = make_store();
        for _ in 0..3 {
            store
                .create_node("g", vec!["N".to_string()], IndexMap::new())
                .unwrap();
        }

        let outer_rows: Vec<Row> = (0..3)
            .map(|_| {
                let mut r = Row::new();
                r.insert("__dummy__".to_string(), Value::Int(0));
                r
            })
            .collect();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("x"),
        };

        let mut budget = ExecutionBudget::new(Duration::from_secs(60), 2);
        let mut stats = MutationStats::default();
        let mut cx = ExecContext {
            graph: "g",
            budget: &mut budget,
            outer_rows: &outer_rows,
            stats: &mut stats,
        };
        let result = executor.exec_node_scan(&mut cx, &plan);
        assert!(
            result.is_err(),
            "expected budget error for Cartesian product exceeding row limit"
        );
    }

    #[test]
    fn index_scan_finds_nodes() {
        let store = make_store();
        let mut p1 = IndexMap::new();
        p1.insert("age".to_string(), Value::Int(30));
        store
            .create_node("g", vec!["Person".to_string()], p1)
            .unwrap();
        store
            .create_node("g", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::IndexScan {
            label: Label::new("Person"),
            property: "age".to_string(),
            value: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn index_scan_empty_when_no_property() {
        let store = make_store();
        store
            .create_node("g", vec!["P".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::IndexScan {
            label: Label::new("P"),
            property: "nonexistent".to_string(),
            value: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn node_by_id_seek_found() {
        let store = make_store();
        let node_id = store
            .create_node("g", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: crate::ast::Expr::Integer(node_id.0 as i64),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn node_by_id_seek_not_found() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(99999),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn node_by_id_seek_negative_id_errors() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(-1),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget);
        assert!(result.is_err());
    }

    #[test]
    fn node_scan_with_outer_rows_merges_context() {
        let store = make_store();
        let nid = store
            .create_node("g", vec!["P".to_string()], IndexMap::new())
            .unwrap();

        let mut outer = Row::new();
        outer.insert("ctx".to_string(), Value::Int(42));

        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let mut cx = ExecContext {
            graph: "g",
            budget: &mut budget,
            outer_rows: &[outer],
            stats: &mut stats,
        };
        let rows = executor.exec_node_scan(&mut cx, &plan).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("ctx"), Some(&Value::Int(42)));
        assert_eq!(rows[0].get("n"), Some(&Value::Node(nid)));
    }
}
