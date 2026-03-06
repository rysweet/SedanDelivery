//! Row-shaping operators: filter, project, distinct, sort, skip, limit.

use crate::error::GqlError;
use crate::executor::budget::ExecutionBudget;
use crate::planner::Operator;
use crate::types::Value;

use super::helpers::{
    cmp_values, eval_count_expr, expr_to_column_name, inject_parameters, RowDedupSet,
};
use super::{MutationStats, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_filter(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Filter { predicate, child } = op else {
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        let mut out = Vec::new();
        for row in input {
            budget.check_or_err()?;
            let ctx = self.build_filter_ctx(graph, &row)?;
            // Use eval_with_storage so that property access on Node/Edge IDs
            // is resolved against the store rather than returning Null.
            let result = self.eval_with_storage(graph, predicate, &row, &ctx)?;
            if result == Value::Bool(true) {
                out.push(row);
            }
        }
        Ok(out)
    }

    pub(crate) fn exec_project(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Project { items, child } = op else {
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        let mut out = Vec::with_capacity(input.len());

        for row in &input {
            budget.check_or_err()?;
            let ctx = self.build_filter_ctx(graph, row)?;
            let mut new_row = Row::new();
            for (expr, alias) in items {
                match self.eval_with_storage(graph, expr, row, &ctx) {
                    Ok(val) => {
                        let col_name = match alias {
                            Some(id) => id.0.clone(),
                            None => expr_to_column_name(expr),
                        };
                        new_row.insert(col_name, val);
                    }
                    Err(e) => {
                        // If we already produced at least one row, treat
                        // this as a streaming error: return the partial
                        // rows and stash the error for the caller.
                        if !out.is_empty() {
                            *self.streaming_error.borrow_mut() = Some(e);
                            return Ok(out);
                        }
                        // No partial rows yet: propagate the error
                        // immediately (standard behaviour).
                        return Err(e);
                    }
                }
            }
            // Preserve query parameters through scope changes.
            inject_parameters(&mut new_row, &self.parameters);
            budget.record_row();
            out.push(new_row);
        }
        Ok(out)
    }

    pub(crate) fn exec_distinct(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Distinct { child } = op else {
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        let mut seen = RowDedupSet::new();
        let mut out = Vec::new();
        for row in input {
            budget.check_or_err()?;
            if seen.insert(&row) {
                out.push(row);
            }
        }
        Ok(out)
    }

    pub(crate) fn exec_sort(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Sort { items, child } = op else {
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        budget.check_or_err()?;

        // Evaluate all sort keys up front to avoid repeated store lookups.
        let mut keyed: Vec<(Vec<Value>, Row)> = input
            .into_iter()
            .map(|row| {
                let ctx = self.build_filter_ctx(graph, &row)?;
                let keys: Result<Vec<Value>, GqlError> = items
                    .iter()
                    .map(|(expr, _)| self.eval_with_storage(graph, expr, &row, &ctx))
                    .collect();
                Ok((keys?, row))
            })
            .collect::<Result<Vec<_>, GqlError>>()?;

        keyed.sort_by(|(ka, _), (kb, _)| {
            for i in 0..items.len() {
                let va = &ka[i];
                let vb = &kb[i];
                let asc = items[i].1;
                let ord = cmp_values(va, vb);
                if ord != std::cmp::Ordering::Equal {
                    return if asc { ord } else { ord.reverse() };
                }
            }
            std::cmp::Ordering::Equal
        });

        Ok(keyed.into_iter().map(|(_, row)| row).collect())
    }

    pub(crate) fn exec_limit(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Limit { count, child } = op else {
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        budget.check_or_err()?;
        let n = eval_count_expr(count, &self.parameters)?;
        Ok(input.into_iter().take(n).collect())
    }

    pub(crate) fn exec_skip(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Skip { count, child } = op else {
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        budget.check_or_err()?;
        let n = eval_count_expr(count, &self.parameters)?;
        Ok(input.into_iter().skip(n).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOp, Expr, Identifier, Label};
    use crate::executor::budget::ExecutionBudget;
    use crate::planner::Operator;
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use std::collections::HashMap;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn filter_keeps_matching() {
        let store = make_store();
        let mut p = HashMap::new();
        p.insert("x".to_string(), Value::Int(10));
        store.create_node("g", vec!["N".to_string()], p).unwrap();
        store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Filter {
            predicate: Expr::BinaryOp {
                op: BinaryOp::Gte,
                left: Box::new(Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("n"))),
                    "x".into(),
                )),
                right: Box::new(Expr::Integer(5)),
            },
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn distinct_removes_duplicates() {
        let store = make_store();
        store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Distinct {
            child: Box::new(Operator::Project {
                items: vec![(Expr::Integer(1), Some(Identifier::new("x")))],
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn sort_ascending() {
        let store = make_store();
        for name in &["C", "A", "B"] {
            let mut p = HashMap::new();
            p.insert("name".to_string(), Value::Str((*name).into()));
            store.create_node("g", vec!["P".to_string()], p).unwrap();
        }
        let executor = QueryExecutor::new(&store);
        let plan = Operator::ProduceResults {
            columns: vec![(
                "n.name".to_string(),
                Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("n"))),
                    "name".into(),
                ),
            )],
            child: Box::new(Operator::Sort {
                items: vec![(
                    Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("n"))),
                        "name".into(),
                    ),
                    true,
                )],
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("P")),
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows[0]["n.name"], Value::Str("A".into()));
        assert_eq!(result.rows[2]["n.name"], Value::Str("C".into()));
    }

    #[test]
    fn limit_and_skip() {
        let store = make_store();
        for _ in 0..5 {
            store.create_node("g", vec![], HashMap::new()).unwrap();
        }
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Limit {
            count: Expr::Integer(2),
            child: Box::new(Operator::Skip {
                count: Expr::Integer(1),
                child: Box::new(Operator::NodeScan {
                    label: None,
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }
}
