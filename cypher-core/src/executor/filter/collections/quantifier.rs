//! Quantifier predicates: `all`, `any`, `none`, `single`.

use std::collections::HashMap;

use crate::ast::Expr;
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::{eval_expr, Row};

/// Convert quantifier counts into the result value.
/// Shared by `eval_quantifier` (filter.rs) and the storage-aware path in engine.rs.
/// `name` must be one of `"all"`, `"any"`, `"none"`, or `"single"` (already lower-cased).
pub(crate) fn quantifier_result(
    name: &str,
    true_count: usize,
    false_count: usize,
    null_count: usize,
) -> Value {
    match name {
        "all" => {
            if false_count > 0 {
                Value::Bool(false)
            } else if null_count > 0 {
                Value::Null
            } else {
                Value::Bool(true)
            }
        }
        "any" => {
            if true_count > 0 {
                Value::Bool(true)
            } else if null_count > 0 {
                Value::Null
            } else {
                Value::Bool(false)
            }
        }
        "none" => {
            if true_count > 0 {
                Value::Bool(false)
            } else if null_count > 0 {
                Value::Null
            } else {
                Value::Bool(true)
            }
        }
        "single" => {
            if true_count > 1 {
                Value::Bool(false)
            } else if null_count > 0 {
                Value::Null
            } else {
                Value::Bool(true_count == 1)
            }
        }
        _ => unreachable!(),
    }
}

/// Evaluate a quantifier predicate: `all`, `any`, `none`, or `single`.
///
/// These take the form `func(x IN list WHERE pred)` which the parser rewrites
/// to `FunctionCall(name, [ListComp { variable, list, filter, map }])`.
/// We must evaluate the predicate for *every* element (not filter/skip) so we
/// can correctly count true/false/null results.
pub(in crate::executor::filter) fn eval_quantifier<'r>(
    name: &str,
    args: &[Expr],
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    if args.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: format!("{name}(variable IN list WHERE predicate)"),
            actual: format!("{} arguments", args.len()),
        }));
    }

    match &args[0] {
        Expr::ListComp {
            variable,
            list,
            filter,
            ..
        } => {
            let list_val = eval_expr(list, row)?;
            match list_val {
                Value::Null => Ok(Value::Null),
                Value::List(items) => {
                    let var_name = variable.0.as_str();
                    let mut true_count: usize = 0;
                    let mut false_count: usize = 0;
                    let mut null_count: usize = 0;

                    for item in &items {
                        let mut extended: HashMap<&str, &Value> =
                            row.iter().map(|(&k, v)| (k, *v)).collect();
                        extended.insert(var_name, item);

                        let result = if let Some(pred) = filter {
                            eval_expr(pred, &extended)?
                        } else {
                            // No WHERE — treat the element itself as the predicate value.
                            item.clone()
                        };

                        match result {
                            Value::Bool(true) => true_count += 1,
                            Value::Bool(false) => false_count += 1,
                            Value::Null => null_count += 1,
                            // Non-boolean values are treated as false.
                            _ => false_count += 1,
                        }
                    }

                    let lc = name.to_ascii_lowercase();
                    Ok(quantifier_result(
                        lc.as_str(),
                        true_count,
                        false_count,
                        null_count,
                    ))
                }
                other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "List".to_string(),
                    actual: other.type_name().to_string(),
                })),
            }
        }
        // Fallback: if the arg is not a ListComp (e.g. `all(someListVar)`), evaluate
        // it as a list of values and check their truthiness.
        other => {
            let val = eval_expr(other, row)?;
            match val {
                Value::Null => Ok(Value::Null),
                Value::List(items) => {
                    let mut true_count: usize = 0;
                    let mut false_count: usize = 0;
                    let mut null_count: usize = 0;

                    for item in &items {
                        match item {
                            Value::Bool(true) => true_count += 1,
                            Value::Bool(false) => false_count += 1,
                            Value::Null => null_count += 1,
                            _ => false_count += 1,
                        }
                    }

                    let lc = name.to_ascii_lowercase();
                    Ok(quantifier_result(
                        lc.as_str(),
                        true_count,
                        false_count,
                        null_count,
                    ))
                }
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "List".to_string(),
                    actual: val.type_name().to_string(),
                })),
            }
        }
    }
}
