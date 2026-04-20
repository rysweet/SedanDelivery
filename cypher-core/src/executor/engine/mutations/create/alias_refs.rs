//! Helpers for detecting whether expressions reference aliases that are
//! still pending creation in the current `CREATE` pattern. These checks
//! drive the flush boundaries between batched node/edge creates so that
//! later items can correctly read earlier-bound aliases.

use std::collections::HashSet;

use crate::ast::Expr;

pub(super) fn expr_references_any_alias(expr: &Expr, aliases: &HashSet<String>) -> bool {
    match expr {
        Expr::Variable(id) => aliases.contains(id.0.as_str()),
        Expr::Property(obj, key) => {
            let _ = key;
            expr_references_any_alias(obj, aliases)
        }
        Expr::BinaryOp { left, right, .. } => {
            expr_references_any_alias(left, aliases) || expr_references_any_alias(right, aliases)
        }
        Expr::UnaryOp { expr, .. }
        | Expr::IsNull(expr)
        | Expr::IsNotNull(expr)
        | Expr::HasLabel(expr, _) => expr_references_any_alias(expr, aliases),
        Expr::FunctionCall { args, .. } | Expr::List(args) => args
            .iter()
            .any(|arg| expr_references_any_alias(arg, aliases)),
        Expr::MapLiteral(entries) => entries
            .iter()
            .any(|(_, value)| expr_references_any_alias(value, aliases)),
        Expr::Case {
            subject,
            alternatives,
            default,
        } => {
            subject
                .as_deref()
                .is_some_and(|expr| expr_references_any_alias(expr, aliases))
                || alternatives.iter().any(|(when, then)| {
                    expr_references_any_alias(when, aliases)
                        || expr_references_any_alias(then, aliases)
                })
                || default
                    .as_deref()
                    .is_some_and(|expr| expr_references_any_alias(expr, aliases))
        }
        Expr::Subscript(list, index) => {
            expr_references_any_alias(list, aliases) || expr_references_any_alias(index, aliases)
        }
        Expr::Slice(list, start, end) => {
            expr_references_any_alias(list, aliases)
                || start
                    .as_deref()
                    .is_some_and(|expr| expr_references_any_alias(expr, aliases))
                || end
                    .as_deref()
                    .is_some_and(|expr| expr_references_any_alias(expr, aliases))
        }
        Expr::ListComp {
            variable,
            list,
            filter,
            map,
        } => {
            if expr_references_any_alias(list, aliases) {
                return true;
            }
            let scoped_aliases: HashSet<String> = aliases
                .iter()
                .filter(|alias| alias.as_str() != variable.0.as_str())
                .cloned()
                .collect();
            filter
                .as_deref()
                .is_some_and(|expr| expr_references_any_alias(expr, &scoped_aliases))
                || expr_references_any_alias(map, &scoped_aliases)
        }
        Expr::Reduce {
            accumulator,
            initial,
            variable,
            list,
            expression,
        } => {
            if expr_references_any_alias(initial, aliases)
                || expr_references_any_alias(list, aliases)
            {
                return true;
            }
            let scoped_aliases: HashSet<String> = aliases
                .iter()
                .filter(|alias| {
                    alias.as_str() != accumulator.0.as_str()
                        && alias.as_str() != variable.0.as_str()
                })
                .cloned()
                .collect();
            expr_references_any_alias(expression, &scoped_aliases)
        }
        Expr::ExistsSubquery {
            where_clause,
            full_query,
            ..
        } => {
            where_clause
                .as_deref()
                .is_some_and(|expr| expr_references_any_alias(expr, aliases))
                || full_query.is_some()
        }
        Expr::PatternExpr(_) => !aliases.is_empty(),
        Expr::Integer(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Bool(_)
        | Expr::Null
        | Expr::Parameter(_) => false,
    }
}

pub(super) fn prop_exprs_reference_any_alias(
    prop_exprs: &[(String, Expr)],
    aliases: &HashSet<String>,
) -> bool {
    prop_exprs
        .iter()
        .any(|(_, expr)| expr_references_any_alias(expr, aliases))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOp, Identifier, Label, UnaryOp};

    fn aliases(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn var(name: &str) -> Expr {
        Expr::Variable(Identifier::new(name))
    }

    #[test]
    fn variable_hits_and_misses() {
        let a = aliases(&["x"]);
        assert!(expr_references_any_alias(&var("x"), &a));
        assert!(!expr_references_any_alias(&var("y"), &a));
    }

    #[test]
    fn literals_never_reference_aliases() {
        let a = aliases(&["x"]);
        assert!(!expr_references_any_alias(&Expr::Integer(1), &a));
        assert!(!expr_references_any_alias(&Expr::Float(1.0), &a));
        assert!(!expr_references_any_alias(&Expr::Str("x".into()), &a));
        assert!(!expr_references_any_alias(&Expr::Bool(true), &a));
        assert!(!expr_references_any_alias(&Expr::Null, &a));
        assert!(!expr_references_any_alias(&Expr::Parameter("p".into()), &a));
    }

    #[test]
    fn property_descends_into_object() {
        let a = aliases(&["x"]);
        let e = Expr::Property(Box::new(var("x")), "name".into());
        assert!(expr_references_any_alias(&e, &a));
        let e2 = Expr::Property(Box::new(var("y")), "name".into());
        assert!(!expr_references_any_alias(&e2, &a));
    }

    #[test]
    fn binary_op_either_side() {
        let a = aliases(&["x"]);
        let e = Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(Expr::Integer(1)),
            right: Box::new(var("x")),
        };
        assert!(expr_references_any_alias(&e, &a));
        let e = Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(var("x")),
            right: Box::new(Expr::Integer(1)),
        };
        assert!(expr_references_any_alias(&e, &a));
    }

    #[test]
    fn unary_isnull_isnotnull_haslabel() {
        let a = aliases(&["x"]);
        let e = Expr::UnaryOp {
            op: UnaryOp::Neg,
            expr: Box::new(var("x")),
        };
        assert!(expr_references_any_alias(&e, &a));
        assert!(expr_references_any_alias(
            &Expr::IsNull(Box::new(var("x"))),
            &a
        ));
        assert!(expr_references_any_alias(
            &Expr::IsNotNull(Box::new(var("x"))),
            &a
        ));
        assert!(expr_references_any_alias(
            &Expr::HasLabel(Box::new(var("x")), Label::new("L")),
            &a
        ));
    }

    #[test]
    fn function_call_and_list_check_args() {
        let a = aliases(&["x"]);
        let e = Expr::FunctionCall {
            name: "f".into(),
            args: vec![Expr::Integer(1), var("x")],
            distinct: false,
        };
        assert!(expr_references_any_alias(&e, &a));
        let e = Expr::List(vec![Expr::Integer(1), var("x")]);
        assert!(expr_references_any_alias(&e, &a));
    }

    #[test]
    fn map_literal_checks_values() {
        let a = aliases(&["x"]);
        let e = Expr::MapLiteral(vec![("k".into(), var("x"))]);
        assert!(expr_references_any_alias(&e, &a));
        let e = Expr::MapLiteral(vec![("k".into(), Expr::Integer(1))]);
        assert!(!expr_references_any_alias(&e, &a));
    }

    #[test]
    fn case_expression_all_parts() {
        let a = aliases(&["x"]);
        // subject references
        let e = Expr::Case {
            subject: Some(Box::new(var("x"))),
            alternatives: vec![],
            default: None,
        };
        assert!(expr_references_any_alias(&e, &a));
        // when references
        let e = Expr::Case {
            subject: None,
            alternatives: vec![(var("x"), Expr::Integer(1))],
            default: None,
        };
        assert!(expr_references_any_alias(&e, &a));
        // then references
        let e = Expr::Case {
            subject: None,
            alternatives: vec![(Expr::Integer(1), var("x"))],
            default: None,
        };
        assert!(expr_references_any_alias(&e, &a));
        // default references
        let e = Expr::Case {
            subject: None,
            alternatives: vec![],
            default: Some(Box::new(var("x"))),
        };
        assert!(expr_references_any_alias(&e, &a));
        // none reference
        let e = Expr::Case {
            subject: None,
            alternatives: vec![(Expr::Integer(1), Expr::Integer(2))],
            default: None,
        };
        assert!(!expr_references_any_alias(&e, &a));
    }

    #[test]
    fn subscript_and_slice_descend() {
        let a = aliases(&["x"]);
        let e = Expr::Subscript(Box::new(Expr::List(vec![])), Box::new(var("x")));
        assert!(expr_references_any_alias(&e, &a));
        let e = Expr::Slice(Box::new(var("x")), None, None);
        assert!(expr_references_any_alias(&e, &a));
        let e = Expr::Slice(Box::new(Expr::List(vec![])), Some(Box::new(var("x"))), None);
        assert!(expr_references_any_alias(&e, &a));
        let e = Expr::Slice(Box::new(Expr::List(vec![])), None, Some(Box::new(var("x"))));
        assert!(expr_references_any_alias(&e, &a));
    }

    #[test]
    fn list_comp_scopes_loop_variable() {
        let a = aliases(&["x"]);
        // map references the loop var (which is "x") → scoped out, so no hit.
        let e = Expr::ListComp {
            variable: Identifier::new("x"),
            list: Box::new(Expr::List(vec![])),
            filter: None,
            map: Box::new(var("x")),
        };
        assert!(!expr_references_any_alias(&e, &a));
        // list itself references → hit (even before scope shadow).
        let e = Expr::ListComp {
            variable: Identifier::new("y"),
            list: Box::new(var("x")),
            filter: None,
            map: Box::new(Expr::Integer(1)),
        };
        assert!(expr_references_any_alias(&e, &a));
        // filter references with non-shadowing var → hit.
        let e = Expr::ListComp {
            variable: Identifier::new("y"),
            list: Box::new(Expr::List(vec![])),
            filter: Some(Box::new(var("x"))),
            map: Box::new(Expr::Integer(1)),
        };
        assert!(expr_references_any_alias(&e, &a));
    }

    #[test]
    fn reduce_scopes_acc_and_var() {
        let a = aliases(&["x"]);
        // expression refers to accumulator name "x" → scoped out → false.
        let e = Expr::Reduce {
            accumulator: Identifier::new("x"),
            initial: Box::new(Expr::Integer(0)),
            variable: Identifier::new("v"),
            list: Box::new(Expr::List(vec![])),
            expression: Box::new(var("x")),
        };
        assert!(!expr_references_any_alias(&e, &a));
        // initial references → hit.
        let e = Expr::Reduce {
            accumulator: Identifier::new("acc"),
            initial: Box::new(var("x")),
            variable: Identifier::new("v"),
            list: Box::new(Expr::List(vec![])),
            expression: Box::new(Expr::Integer(0)),
        };
        assert!(expr_references_any_alias(&e, &a));
        // list references → hit.
        let e = Expr::Reduce {
            accumulator: Identifier::new("acc"),
            initial: Box::new(Expr::Integer(0)),
            variable: Identifier::new("v"),
            list: Box::new(var("x")),
            expression: Box::new(Expr::Integer(0)),
        };
        assert!(expr_references_any_alias(&e, &a));
    }

    #[test]
    fn exists_subquery_and_pattern_expr() {
        let a = aliases(&["x"]);
        // PatternExpr is conservative: any non-empty alias set yields true.
        assert!(expr_references_any_alias(&Expr::PatternExpr(vec![]), &a));
        assert!(!expr_references_any_alias(
            &Expr::PatternExpr(vec![]),
            &HashSet::new()
        ));
        // ExistsSubquery: where_clause referencing aliases → hit.
        let e = Expr::ExistsSubquery {
            patterns: vec![],
            where_clause: Some(Box::new(var("x"))),
            full_query: None,
        };
        assert!(expr_references_any_alias(&e, &a));
        // ExistsSubquery without where but with full_query → conservative true.
        let q = crate::ast::Query {
            unions: vec![],
            clauses: vec![],
        };
        let e = Expr::ExistsSubquery {
            patterns: vec![],
            where_clause: None,
            full_query: Some(Box::new(q)),
        };
        assert!(expr_references_any_alias(&e, &a));
    }

    #[test]
    fn prop_exprs_helper_aggregates() {
        let a = aliases(&["x"]);
        let props = vec![
            ("a".to_string(), Expr::Integer(1)),
            ("b".to_string(), var("x")),
        ];
        assert!(prop_exprs_reference_any_alias(&props, &a));
        let props = vec![("a".to_string(), Expr::Integer(1))];
        assert!(!prop_exprs_reference_any_alias(&props, &a));
    }
}
