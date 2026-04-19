//! `NodeByIdSeek` operator: looks up a single node by its internal ID.

use std::collections::HashMap;

use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::eval_expr;
use crate::executor::filter::Row as FilterRow;
use crate::planner::Operator;
use crate::storage::NodeId;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
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
