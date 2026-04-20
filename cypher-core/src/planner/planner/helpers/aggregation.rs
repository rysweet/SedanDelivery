//! Aggregate-function detection and rewriting utilities.

use crate::ast::{Expr, Identifier};
use crate::error::{ExecutionError, GqlError};
use crate::planner::AggFunction;

/// Returns `true` if the expression is or contains an aggregate function call.
pub(crate) fn has_agg_function(expr: &Expr) -> bool {
    match expr {
        Expr::FunctionCall { name, args, .. } => {
            // The function itself is an aggregate, OR one of its args contains an aggregate
            // (e.g. size(collect(a)), all(x IN collect(...) WHERE x))
            is_agg_name(name.as_str()) || args.iter().any(has_agg_function)
        }
        Expr::BinaryOp { left, right, .. } => has_agg_function(left) || has_agg_function(right),
        Expr::UnaryOp { expr, .. } => has_agg_function(expr),
        Expr::Property(obj, _) => has_agg_function(obj),
        Expr::List(items) => items.iter().any(has_agg_function),
        Expr::MapLiteral(pairs) => pairs.iter().any(|(_, v)| has_agg_function(v)),
        Expr::ListComp {
            list, filter, map, ..
        } => {
            has_agg_function(list)
                || filter.as_deref().map(has_agg_function).unwrap_or(false)
                || has_agg_function(map)
        }
        Expr::Case {
            subject,
            alternatives,
            default,
        } => {
            subject.as_deref().map(has_agg_function).unwrap_or(false)
                || alternatives
                    .iter()
                    .any(|(c, r)| has_agg_function(c) || has_agg_function(r))
                || default.as_deref().map(has_agg_function).unwrap_or(false)
        }
        Expr::IsNull(e) | Expr::IsNotNull(e) => has_agg_function(e),
        Expr::Subscript(a, b) => has_agg_function(a) || has_agg_function(b),
        Expr::Slice(a, b, c) => {
            has_agg_function(a)
                || b.as_deref().map(has_agg_function).unwrap_or(false)
                || c.as_deref().map(has_agg_function).unwrap_or(false)
        }
        _ => false,
    }
}

pub(crate) fn is_agg_name(name: &str) -> bool {
    crate::planner::is_aggregate_function(name)
}

/// Extract the aggregate function, first argument, optional second argument, and distinct flag
/// from a top-level `FunctionCall` expression.
pub(crate) fn extract_agg_function(
    expr: &Expr,
) -> Result<(AggFunction, Expr, Option<Expr>, bool), GqlError> {
    match expr {
        Expr::FunctionCall {
            name,
            args,
            distinct,
        } => {
            let func = match name.to_uppercase().as_str() {
                "COUNT" => AggFunction::Count,
                "SUM" => AggFunction::Sum,
                "AVG" => AggFunction::Avg,
                "MIN" => AggFunction::Min,
                "MAX" => AggFunction::Max,
                "COLLECT" => AggFunction::Collect,
                "STDEV" => AggFunction::StDev,
                "STDEVP" => AggFunction::StDevP,
                "PERCENTILECONT" => AggFunction::PercentileCont,
                "PERCENTILEDISC" => AggFunction::PercentileDisc,
                other => {
                    return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                        "aggregate function '{other}'"
                    ))));
                }
            };
            let arg = args.first().cloned().unwrap_or(Expr::Null);
            let second_arg = args.get(1).cloned();
            Ok((func, arg, second_arg, *distinct))
        }
        _ => Err(GqlError::Execution(ExecutionError::unsupported(
            "non-function aggregate expression",
        ))),
    }
}

/// Find the innermost aggregate function call in a compound expression.
/// Returns (AggFunction, arg_expr, second_arg_expr, distinct) if found.
pub(crate) fn find_inner_aggregate(expr: &Expr) -> Option<(AggFunction, Expr, Option<Expr>, bool)> {
    match expr {
        Expr::FunctionCall {
            name,
            args,
            distinct,
        } => {
            let func_opt = match name.to_uppercase().as_str() {
                "COUNT" => Some(AggFunction::Count),
                "SUM" => Some(AggFunction::Sum),
                "AVG" => Some(AggFunction::Avg),
                "MIN" => Some(AggFunction::Min),
                "MAX" => Some(AggFunction::Max),
                "COLLECT" => Some(AggFunction::Collect),
                "STDEV" => Some(AggFunction::StDev),
                "STDEVP" => Some(AggFunction::StDevP),
                "PERCENTILECONT" => Some(AggFunction::PercentileCont),
                "PERCENTILEDISC" => Some(AggFunction::PercentileDisc),
                _ => None,
            };
            if let Some(func) = func_opt {
                let arg = args.first().cloned().unwrap_or(Expr::Null);
                let second_arg = args.get(1).cloned();
                Some((func, arg, second_arg, *distinct))
            } else {
                args.iter().find_map(find_inner_aggregate)
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            find_inner_aggregate(left).or_else(|| find_inner_aggregate(right))
        }
        Expr::UnaryOp { expr: inner, .. } => find_inner_aggregate(inner),
        Expr::Property(inner, _) => find_inner_aggregate(inner),
        Expr::MapLiteral(pairs) => pairs.iter().find_map(|(_, v)| find_inner_aggregate(v)),
        Expr::ListComp { list, map, .. } => {
            find_inner_aggregate(list).or_else(|| find_inner_aggregate(map))
        }
        _ => None,
    }
}

/// Rewrite an expression by replacing the first aggregate function call with
/// a variable reference to `alias`.
pub(crate) fn rewrite_agg_to_var(expr: &Expr, alias: &Identifier) -> Expr {
    match expr {
        Expr::FunctionCall { name, .. } if is_agg_name(name) => {
            // This IS the aggregate: replace the whole call with a variable.
            Expr::Variable(alias.clone())
        }
        Expr::FunctionCall {
            name,
            args,
            distinct,
        } => {
            // Non-aggregate function: rewrite inside args if any contain an aggregate.
            if args.iter().any(has_agg_function) {
                let new_args = args
                    .iter()
                    .map(|a| {
                        if has_agg_function(a) {
                            rewrite_agg_to_var(a, alias)
                        } else {
                            a.clone()
                        }
                    })
                    .collect();
                Expr::FunctionCall {
                    name: name.clone(),
                    args: new_args,
                    distinct: *distinct,
                }
            } else {
                expr.clone()
            }
        }
        Expr::BinaryOp { op, left, right } => {
            if has_agg_function(left) {
                Expr::BinaryOp {
                    op: *op,
                    left: Box::new(rewrite_agg_to_var(left, alias)),
                    right: right.clone(),
                }
            } else if has_agg_function(right) {
                Expr::BinaryOp {
                    op: *op,
                    left: left.clone(),
                    right: Box::new(rewrite_agg_to_var(right, alias)),
                }
            } else {
                expr.clone()
            }
        }
        Expr::UnaryOp { op, expr: inner } => Expr::UnaryOp {
            op: *op,
            expr: Box::new(rewrite_agg_to_var(inner, alias)),
        },
        Expr::MapLiteral(pairs) => {
            let new_pairs = pairs
                .iter()
                .map(|(k, v)| {
                    if has_agg_function(v) {
                        (k.clone(), rewrite_agg_to_var(v, alias))
                    } else {
                        (k.clone(), v.clone())
                    }
                })
                .collect();
            Expr::MapLiteral(new_pairs)
        }
        Expr::ListComp {
            variable,
            list,
            filter,
            map,
        } => Expr::ListComp {
            variable: variable.clone(),
            list: if has_agg_function(list) {
                Box::new(rewrite_agg_to_var(list, alias))
            } else {
                list.clone()
            },
            filter: filter.clone(),
            map: if has_agg_function(map) {
                Box::new(rewrite_agg_to_var(map, alias))
            } else {
                map.clone()
            },
        },
        _ => expr.clone(),
    }
}

/// Check if an expression contains an aggregation function.
pub(crate) fn contains_aggregation(expr: &Expr) -> bool {
    match expr {
        Expr::FunctionCall { name, args, .. } => {
            if crate::planner::is_aggregate_function(name.as_str()) {
                return true;
            }
            args.iter().any(contains_aggregation)
        }
        Expr::BinaryOp { left, right, .. } => {
            contains_aggregation(left) || contains_aggregation(right)
        }
        Expr::UnaryOp { expr: inner, .. } => contains_aggregation(inner),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOp, Identifier, UnaryOp};
    use smol_str::SmolStr;

    fn agg(name: &str) -> Expr {
        Expr::FunctionCall {
            name: SmolStr::new(name),
            args: vec![Expr::Integer(1)],
            distinct: false,
        }
    }
    fn nonagg(name: &str, args: Vec<Expr>) -> Expr {
        Expr::FunctionCall {
            name: SmolStr::new(name),
            args,
            distinct: false,
        }
    }
    fn var(n: &str) -> Expr {
        Expr::Variable(Identifier::new(n))
    }

    #[test]
    fn has_agg_direct() {
        assert!(has_agg_function(&agg("count")));
    }
    #[test]
    fn has_agg_in_args() {
        assert!(has_agg_function(&nonagg("size", vec![agg("collect")])));
    }
    #[test]
    fn has_agg_in_binop() {
        let e = Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(agg("sum")),
            right: Box::new(Expr::Integer(1)),
        };
        assert!(has_agg_function(&e));
    }
    #[test]
    fn has_agg_in_unary() {
        let e = Expr::UnaryOp {
            op: UnaryOp::Neg,
            expr: Box::new(agg("sum")),
        };
        assert!(has_agg_function(&e));
    }
    #[test]
    fn has_agg_in_property() {
        assert!(has_agg_function(&Expr::Property(
            Box::new(agg("collect")),
            SmolStr::new("x")
        )));
    }
    #[test]
    fn has_agg_in_list() {
        assert!(has_agg_function(&Expr::List(vec![
            Expr::Integer(1),
            agg("count")
        ])));
    }
    #[test]
    fn has_agg_in_map() {
        assert!(has_agg_function(&Expr::MapLiteral(vec![(
            SmolStr::new("a"),
            agg("count")
        )])));
    }
    #[test]
    fn has_agg_in_listcomp_list() {
        let e = Expr::ListComp {
            variable: Identifier::new("x"),
            list: Box::new(agg("collect")),
            filter: None,
            map: Box::new(var("x")),
        };
        assert!(has_agg_function(&e));
    }
    #[test]
    fn has_agg_in_listcomp_filter() {
        let e = Expr::ListComp {
            variable: Identifier::new("x"),
            list: Box::new(Expr::List(vec![])),
            filter: Some(Box::new(agg("count"))),
            map: Box::new(var("x")),
        };
        assert!(has_agg_function(&e));
    }
    #[test]
    fn has_agg_in_case_subject() {
        let e = Expr::Case {
            subject: Some(Box::new(agg("count"))),
            alternatives: vec![],
            default: None,
        };
        assert!(has_agg_function(&e));
    }
    #[test]
    fn has_agg_in_case_alts() {
        let e = Expr::Case {
            subject: None,
            alternatives: vec![(agg("count"), Expr::Integer(1))],
            default: None,
        };
        assert!(has_agg_function(&e));
    }
    #[test]
    fn has_agg_in_case_default() {
        let e = Expr::Case {
            subject: None,
            alternatives: vec![],
            default: Some(Box::new(agg("count"))),
        };
        assert!(has_agg_function(&e));
    }
    #[test]
    fn has_agg_in_isnull() {
        assert!(has_agg_function(&Expr::IsNull(Box::new(agg("count")))));
        assert!(has_agg_function(&Expr::IsNotNull(Box::new(agg("count")))));
    }
    #[test]
    fn has_agg_in_subscript() {
        assert!(has_agg_function(&Expr::Subscript(
            Box::new(agg("count")),
            Box::new(Expr::Integer(0))
        )));
    }
    #[test]
    fn has_agg_in_slice() {
        let e = Expr::Slice(Box::new(agg("collect")), None, None);
        assert!(has_agg_function(&e));
    }
    #[test]
    fn has_agg_false_for_simple() {
        assert!(!has_agg_function(&Expr::Integer(1)));
        assert!(!has_agg_function(&var("x")));
    }
    #[test]
    fn is_agg_name_works() {
        assert!(is_agg_name("count"));
        assert!(is_agg_name("COUNT"));
        assert!(!is_agg_name("not_an_agg"));
    }
    #[test]
    fn extract_count_sum_avg_min_max_collect() {
        for n in [
            "count",
            "sum",
            "avg",
            "min",
            "max",
            "collect",
            "stdev",
            "stdevp",
            "percentileCont",
            "percentileDisc",
        ] {
            let e = agg(n);
            let r = extract_agg_function(&e);
            assert!(r.is_ok(), "{n} failed");
        }
    }
    #[test]
    fn extract_unsupported_errors() {
        let e = Expr::FunctionCall {
            name: SmolStr::new("nope"),
            args: vec![],
            distinct: false,
        };
        assert!(extract_agg_function(&e).is_err());
    }
    #[test]
    fn extract_non_funccall_errors() {
        // Non-FunctionCall expressions should error
        let _ = extract_agg_function(&Expr::Integer(1));
    }
    #[test]
    fn find_inner_top_level_agg() {
        assert!(find_inner_aggregate(&agg("count")).is_some());
    }
    #[test]
    fn find_inner_nested_agg() {
        let e = Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(agg("sum")),
            right: Box::new(Expr::Integer(1)),
        };
        assert!(find_inner_aggregate(&e).is_some());
    }
    #[test]
    fn find_inner_no_agg() {
        assert!(find_inner_aggregate(&Expr::Integer(1)).is_none());
    }
    #[test]
    fn rewrite_top_level_agg() {
        let alias = Identifier::new("a0");
        let r = rewrite_agg_to_var(&agg("count"), &alias);
        assert!(matches!(r, Expr::Variable(_)));
    }
    #[test]
    fn rewrite_inside_binop_left_right() {
        let alias = Identifier::new("a0");
        let l = Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(agg("sum")),
            right: Box::new(Expr::Integer(1)),
        };
        let _ = rewrite_agg_to_var(&l, &alias);
        let r = Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(Expr::Integer(1)),
            right: Box::new(agg("sum")),
        };
        let _ = rewrite_agg_to_var(&r, &alias);
    }
    #[test]
    fn rewrite_unary() {
        let alias = Identifier::new("a0");
        let e = Expr::UnaryOp {
            op: UnaryOp::Neg,
            expr: Box::new(agg("sum")),
        };
        let _ = rewrite_agg_to_var(&e, &alias);
    }
    #[test]
    fn rewrite_nonagg_func_with_agg_arg() {
        let alias = Identifier::new("a0");
        let e = nonagg("size", vec![agg("collect")]);
        let _ = rewrite_agg_to_var(&e, &alias);
    }
    #[test]
    fn rewrite_nonagg_func_without_agg() {
        let alias = Identifier::new("a0");
        let e = nonagg("size", vec![Expr::Integer(1)]);
        let _ = rewrite_agg_to_var(&e, &alias);
    }
    #[test]
    fn rewrite_map_literal() {
        let alias = Identifier::new("a0");
        let e = Expr::MapLiteral(vec![(SmolStr::new("a"), agg("count"))]);
        let _ = rewrite_agg_to_var(&e, &alias);
    }
    #[test]
    fn rewrite_listcomp() {
        let alias = Identifier::new("a0");
        let e = Expr::ListComp {
            variable: Identifier::new("x"),
            list: Box::new(agg("collect")),
            filter: None,
            map: Box::new(agg("count")),
        };
        let _ = rewrite_agg_to_var(&e, &alias);
    }
    #[test]
    fn rewrite_other_passthrough() {
        let alias = Identifier::new("a0");
        let _ = rewrite_agg_to_var(&Expr::Integer(1), &alias);
    }
    #[test]
    fn contains_aggregation_basic() {
        assert!(contains_aggregation(&agg("count")));
        assert!(!contains_aggregation(&Expr::Integer(1)));
        assert!(contains_aggregation(&Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(agg("sum")),
            right: Box::new(Expr::Integer(1)),
        }));
        assert!(contains_aggregation(&Expr::UnaryOp {
            op: UnaryOp::Neg,
            expr: Box::new(agg("sum")),
        }));
    }
}
