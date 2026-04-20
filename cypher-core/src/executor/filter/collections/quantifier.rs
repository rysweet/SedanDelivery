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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, Identifier};
    use std::collections::HashMap;

    fn empty_row<'a>() -> Row<'a> {
        HashMap::new()
    }

    // ── quantifier_result truth tables ─────────────────────────────────────

    #[test]
    fn quantifier_result_all_variants() {
        assert_eq!(quantifier_result("all", 3, 0, 0), Value::Bool(true));
        assert_eq!(quantifier_result("all", 2, 1, 0), Value::Bool(false));
        assert_eq!(quantifier_result("all", 2, 0, 1), Value::Null);
        assert_eq!(quantifier_result("all", 0, 0, 0), Value::Bool(true));
        // false dominates null.
        assert_eq!(quantifier_result("all", 0, 1, 1), Value::Bool(false));
    }

    #[test]
    fn quantifier_result_any_variants() {
        assert_eq!(quantifier_result("any", 1, 5, 0), Value::Bool(true));
        assert_eq!(quantifier_result("any", 0, 0, 1), Value::Null);
        assert_eq!(quantifier_result("any", 0, 5, 0), Value::Bool(false));
        assert_eq!(quantifier_result("any", 0, 0, 0), Value::Bool(false));
    }

    #[test]
    fn quantifier_result_none_variants() {
        assert_eq!(quantifier_result("none", 0, 5, 0), Value::Bool(true));
        assert_eq!(quantifier_result("none", 1, 0, 0), Value::Bool(false));
        assert_eq!(quantifier_result("none", 0, 0, 2), Value::Null);
        assert_eq!(quantifier_result("none", 0, 0, 0), Value::Bool(true));
    }

    #[test]
    fn quantifier_result_single_variants() {
        assert_eq!(quantifier_result("single", 1, 0, 0), Value::Bool(true));
        assert_eq!(quantifier_result("single", 2, 0, 0), Value::Bool(false));
        assert_eq!(quantifier_result("single", 0, 1, 0), Value::Bool(false));
        assert_eq!(quantifier_result("single", 1, 0, 1), Value::Null);
        assert_eq!(quantifier_result("single", 0, 0, 1), Value::Null);
    }

    // ── eval_quantifier dispatch and arity errors ───────────────────────────

    #[test]
    fn eval_quantifier_arity_error() {
        let row = empty_row();
        let err = eval_quantifier("all", &[], &row).unwrap_err();
        assert!(matches!(
            err,
            GqlError::Execution(ExecutionError::TypeMismatch { .. })
        ));
        let args = [Expr::Integer(1), Expr::Integer(2)];
        let err = eval_quantifier("any", &args, &row).unwrap_err();
        assert!(matches!(
            err,
            GqlError::Execution(ExecutionError::TypeMismatch { .. })
        ));
    }

    fn list_comp(items: Vec<Expr>, pred: Option<Expr>) -> Expr {
        Expr::ListComp {
            variable: Identifier("x".to_string()),
            list: Box::new(Expr::List(items)),
            filter: pred.map(Box::new),
            map: Box::new(Expr::Variable(Identifier("x".to_string()))),
        }
    }

    #[test]
    fn eval_quantifier_listcomp_paths() {
        let row = empty_row();
        // all() over [1,2,3] WHERE x > 0 → true
        let pred = Expr::BinaryOp {
            op: crate::ast::BinaryOp::Gt,
            left: Box::new(Expr::Variable(Identifier("x".to_string()))),
            right: Box::new(Expr::Integer(0)),
        };
        let arg = list_comp(
            vec![Expr::Integer(1), Expr::Integer(2), Expr::Integer(3)],
            Some(pred),
        );
        let r = eval_quantifier("all", std::slice::from_ref(&arg), &row).unwrap();
        assert_eq!(r, Value::Bool(true));

        // any() over [1,2] no filter → values are non-bool numerics → counted as false → returns false
        let arg = list_comp(vec![Expr::Integer(1), Expr::Integer(2)], None);
        let r = eval_quantifier("any", std::slice::from_ref(&arg), &row).unwrap();
        assert_eq!(r, Value::Bool(false));

        // single() over [true, true] no filter → 2 trues → false
        let arg = list_comp(vec![Expr::Bool(true), Expr::Bool(true)], None);
        let r = eval_quantifier("single", std::slice::from_ref(&arg), &row).unwrap();
        assert_eq!(r, Value::Bool(false));

        // none() over [false, false] no filter → 0 trues → true
        let arg = list_comp(vec![Expr::Bool(false), Expr::Bool(false)], None);
        let r = eval_quantifier("none", std::slice::from_ref(&arg), &row).unwrap();
        assert_eq!(r, Value::Bool(true));

        // List with null predicate result yields the null branch for `any`.
        let arg = list_comp(vec![Expr::Null, Expr::Null], None);
        let r = eval_quantifier("any", std::slice::from_ref(&arg), &row).unwrap();
        assert_eq!(r, Value::Null);
    }

    #[test]
    fn eval_quantifier_null_list() {
        let row = empty_row();
        // ListComp over a null list: list evaluates to Null first → returns Null.
        let arg = Expr::ListComp {
            variable: Identifier("x".to_string()),
            list: Box::new(Expr::Null),
            filter: None,
            map: Box::new(Expr::Variable(Identifier("x".to_string()))),
        };
        assert_eq!(
            eval_quantifier("all", std::slice::from_ref(&arg), &row).unwrap(),
            Value::Null,
        );
    }

    #[test]
    fn eval_quantifier_non_list_arg_errors() {
        let row = empty_row();
        // Fallback path: a scalar (not a list) → TypeMismatch error.
        let arg = Expr::Integer(42);
        let err = eval_quantifier("all", std::slice::from_ref(&arg), &row).unwrap_err();
        assert!(matches!(
            err,
            GqlError::Execution(ExecutionError::TypeMismatch { .. })
        ));
    }

    #[test]
    fn eval_quantifier_fallback_list_path() {
        let row = empty_row();
        // Plain list of mixed bools/nulls — fallback (non-ListComp) branch.
        let arg = Expr::List(vec![Expr::Bool(true), Expr::Bool(false), Expr::Null]);
        assert_eq!(
            eval_quantifier("any", std::slice::from_ref(&arg), &row).unwrap(),
            Value::Bool(true),
        );
        let arg = Expr::List(vec![Expr::Bool(false), Expr::Null]);
        assert_eq!(
            eval_quantifier("any", std::slice::from_ref(&arg), &row).unwrap(),
            Value::Null,
        );
        let arg = Expr::List(vec![Expr::Bool(false), Expr::Bool(false)]);
        assert_eq!(
            eval_quantifier("none", std::slice::from_ref(&arg), &row).unwrap(),
            Value::Bool(true),
        );
        // Non-bool/null elements counted as false in fallback path.
        let arg = Expr::List(vec![Expr::Integer(1), Expr::Integer(2)]);
        assert_eq!(
            eval_quantifier("all", std::slice::from_ref(&arg), &row).unwrap(),
            Value::Bool(false),
        );
        // Null fallback list → null
        let arg = Expr::Null;
        assert_eq!(
            eval_quantifier("all", std::slice::from_ref(&arg), &row).unwrap(),
            Value::Null,
        );
    }
}
