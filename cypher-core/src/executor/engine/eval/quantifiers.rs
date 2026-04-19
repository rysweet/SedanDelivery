//! Quantifier and looping evaluators split out of `eval`:
//! * `all/any/none/single` (function-call form over a list comprehension arg)
//! * `Expr::ListComp`
//! * `Expr::Reduce`

use crate::ast::{Expr, Identifier};
use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::{eval_expr, quantifier_result, Row as FilterRow};
use crate::types::Value;

use super::super::QueryExecutor;
use super::super::Row;

impl<'a> QueryExecutor<'a> {
    /// Evaluate the function-call form of `all`/`any`/`none`/`single`,
    /// which take a single list-comprehension argument.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn eval_quantifier_func(
        &self,
        graph: &str,
        name: &smol_str::SmolStr,
        args: &[Expr],
        row: &Row,
        ctx: &FilterRow<'_>,
        full_expr: &Expr,
    ) -> Result<Value, GqlError> {
        if args.len() == 1 {
            if let Expr::ListComp {
                variable,
                list,
                filter,
                ..
            } = &args[0]
            {
                let list_val = self.eval_with_storage(graph, list, row, ctx)?;
                let items = match list_val {
                    Value::Null => return Ok(Value::Null),
                    Value::List(items) => items,
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "List".to_string(),
                            actual: other.type_name().to_string(),
                        }));
                    }
                };
                let lc = name.to_lowercase();
                let mut true_count: usize = 0;
                let mut false_count: usize = 0;
                let mut null_count: usize = 0;
                for item in &items {
                    let mut ext_row = row.clone();
                    ext_row.insert(variable.0.clone(), item.clone());
                    let ext_ctx = self.build_filter_ctx(graph, &ext_row)?;
                    let pred_result = if let Some(pred) = filter {
                        self.eval_with_storage(graph, pred, &ext_row, &ext_ctx)?
                    } else {
                        item.clone()
                    };
                    match pred_result {
                        Value::Bool(true) => true_count += 1,
                        Value::Bool(false) => false_count += 1,
                        v if v.is_null() => null_count += 1,
                        _ => false_count += 1,
                    }
                    match lc.as_str() {
                        "all" if false_count > 0 => return Ok(Value::Bool(false)),
                        "any" if true_count > 0 => return Ok(Value::Bool(true)),
                        "none" if true_count > 0 => return Ok(Value::Bool(false)),
                        "single" if true_count > 1 => return Ok(Value::Bool(false)),
                        _ => {}
                    }
                }
                return Ok(quantifier_result(
                    lc.as_str(),
                    true_count,
                    false_count,
                    null_count,
                ));
            }
        }
        eval_expr(full_expr, ctx)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn eval_list_comp(
        &self,
        graph: &str,
        variable: &Identifier,
        list: &Expr,
        filter: Option<&Expr>,
        map: &Expr,
        row: &Row,
        ctx: &FilterRow<'_>,
    ) -> Result<Value, GqlError> {
        let list_val = self.eval_with_storage(graph, list, row, ctx)?;
        match list_val {
            Value::Null => Ok(Value::Null),
            Value::List(items) => {
                let mut result = Vec::with_capacity(items.len());
                for item in &items {
                    let mut extended_row = row.clone();
                    extended_row.insert(variable.0.clone(), item.clone());
                    let ext_ctx = self.build_filter_ctx(graph, &extended_row)?;
                    if let Some(pred) = filter {
                        match self.eval_with_storage(graph, pred, &extended_row, &ext_ctx)? {
                            Value::Bool(true) => {}
                            _ => continue,
                        }
                    }
                    result.push(self.eval_with_storage(graph, map, &extended_row, &ext_ctx)?);
                }
                Ok(Value::List(result))
            }
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "List".to_string(),
                actual: other.type_name().to_string(),
            })),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn eval_reduce(
        &self,
        graph: &str,
        accumulator: &Identifier,
        initial: &Expr,
        variable: &Identifier,
        list: &Expr,
        expression: &Expr,
        row: &Row,
        ctx: &FilterRow<'_>,
    ) -> Result<Value, GqlError> {
        let init_val = self.eval_with_storage(graph, initial, row, ctx)?;
        let list_val = self.eval_with_storage(graph, list, row, ctx)?;
        match list_val {
            Value::Null => Ok(Value::Null),
            Value::List(items) => {
                let mut acc = init_val;
                for item in &items {
                    let mut extended_row = row.clone();
                    extended_row.insert(accumulator.0.clone(), acc);
                    extended_row.insert(variable.0.clone(), item.clone());
                    let ext_ctx = self.build_filter_ctx(graph, &extended_row)?;
                    acc = self.eval_with_storage(graph, expression, &extended_row, &ext_ctx)?;
                }
                Ok(acc)
            }
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "List".to_string(),
                actual: other.type_name().to_string(),
            })),
        }
    }
}
