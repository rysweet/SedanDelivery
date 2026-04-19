//! `NodeScan` operator: scans all nodes (optionally filtered by label).

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
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
}
