//! `IndexScan` operator: looks up nodes by label + property [+ value].

use crate::error::{ExecutionError, GqlError, StorageError};
use crate::executor::filter::eval_expr;
use crate::planner::Operator;
use crate::storage::{NodeId, RangeQuery};
use crate::types::Value;

use crate::executor::engine::helpers::inject_parameters;
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
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
            // If the value expression evaluated to NULL (e.g. property access on
            // a null node), no node can match — return empty immediately.
            if matches!(evaluated_value, Some(Value::Null)) {
                return Ok(vec![]);
            }
            let range_query = match evaluated_value {
                Some(v) => RangeQuery::Eq(v),
                None => RangeQuery::Gte(&Value::Null),
            };
            let store_result = self.store.find_nodes_by_label_and_property(
                cx.graph,
                label.as_str(),
                property,
                range_query,
            );
            match store_result {
                Ok(node_ids) => Ok(node_ids),
                Err(StorageError::NoPropertyIndex {
                    label: l,
                    property: p,
                }) => {
                    // Stores MUST implement find_nodes_by_label_and_property
                    // internally (even without declared indexes). The executor
                    // must NOT fall back to a full label scan — that creates a
                    // cartesian product in UNWIND + Apply contexts (#1090).
                    Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                        feature: format!(
                            "Store returned NoPropertyIndex for {l}.{p} — \
                             store must handle label+property scan internally"
                        ),
                    }))
                }
                Err(e) => Err(e.into()),
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
}
