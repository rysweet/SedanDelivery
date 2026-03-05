//! List comprehension, subscript, slice, quantifier predicates (ALL, ANY, NONE, SINGLE).

use std::collections::HashMap;

use crate::ast::{Expr, Identifier};
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::{eval_expr, Row};

// ── Subscript ─────────────────────────────────────────────────────────────────

pub(super) fn eval_subscript<'r>(
    list_expr: &Expr,
    index_expr: &Expr,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    let list = eval_expr(list_expr, row)?;
    let idx = eval_expr(index_expr, row)?;
    match (list, idx) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::List(items), Value::Int(i)) => {
            let len = items.len();
            // Cypher: negative indices count from the end.
            // Out-of-bounds returns null in Cypher.
            let actual = if i < 0 {
                let offset = (-i) as usize;
                if offset > len {
                    return Ok(Value::Null);
                }
                len - offset
            } else {
                i as usize
            };
            if actual >= len {
                return Ok(Value::Null);
            }
            Ok(items[actual].clone())
        }
        // Map[String] — dynamic property access: map['key']
        (Value::Map(m), Value::Str(key)) => Ok(m.get(key.as_ref()).cloned().unwrap_or(Value::Null)),
        (list, idx) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "List[Integer] or Map[String]".to_string(),
            actual: format!("{}[{}]", list.type_name(), idx.type_name()),
        })),
    }
}

// ── Slice ─────────────────────────────────────────────────────────────────────

pub(super) fn eval_slice<'r>(
    list_expr: &Expr,
    start_expr: &Option<Box<Expr>>,
    end_expr: &Option<Box<Expr>>,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    let list = eval_expr(list_expr, row)?;
    match list {
        Value::Null => Ok(Value::Null),
        Value::List(items) => {
            let len = items.len() as i64;
            // Cypher: null slice bounds → null result
            let start = match start_expr {
                Some(e) => match eval_expr(e, row)? {
                    Value::Int(i) => {
                        // Negative indices count from end
                        let idx = if i < 0 { (len + i).max(0) } else { i.min(len) };
                        idx as usize
                    }
                    Value::Null => return Ok(Value::Null),
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "Integer".to_string(),
                            actual: other.type_name().to_string(),
                        }))
                    }
                },
                None => 0,
            };
            let end = match end_expr {
                Some(e) => match eval_expr(e, row)? {
                    Value::Int(i) => {
                        let idx = if i < 0 { (len + i).max(0) } else { i.min(len) };
                        idx as usize
                    }
                    Value::Null => return Ok(Value::Null),
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "Integer".to_string(),
                            actual: other.type_name().to_string(),
                        }))
                    }
                },
                None => len as usize,
            };
            // Invalid range (start > end) → empty list
            if start >= end {
                Ok(Value::List(vec![]))
            } else {
                Ok(Value::List(items[start..end.min(items.len())].to_vec()))
            }
        }
        other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "List".to_string(),
            actual: other.type_name().to_string(),
        })),
    }
}

// ── List comprehension ────────────────────────────────────────────────────────

pub(super) fn eval_list_comp<'r>(
    variable: &Identifier,
    list: &Expr,
    filter: &Option<Box<Expr>>,
    map: &Expr,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    let list_val = eval_expr(list, row)?;
    match list_val {
        Value::Null => Ok(Value::Null),
        Value::List(items) => {
            let var_name: &str = variable.0.as_str();
            let mut result = Vec::with_capacity(items.len());
            for item in &items {
                // Build an extended row with the loop variable bound.
                let mut extended: HashMap<&str, &Value> =
                    row.iter().map(|(&k, v)| (k, *v)).collect();
                extended.insert(var_name, item);

                // Evaluate filter if present; skip element on false/null.
                if let Some(pred) = filter {
                    match eval_expr(pred, &extended)? {
                        Value::Bool(true) => {}
                        Value::Bool(false) | Value::Null => continue,
                        other => {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Boolean".to_string(),
                                actual: other.type_name().to_string(),
                            }))
                        }
                    }
                }

                // Map the surviving element.
                result.push(eval_expr(map, &extended)?);
            }
            Ok(Value::List(result))
        }
        other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "List".to_string(),
            actual: other.type_name().to_string(),
        })),
    }
}

// ── Quantifier predicates ─────────────────────────────────────────────────────

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
pub(super) fn eval_quantifier<'r>(
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
