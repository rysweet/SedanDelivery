//! Tests for `UNWIND` planning.

use super::*;
use crate::ast::{Clause, Expr, Identifier, Query, ReturnClause, ReturnItem, UnwindClause};

#[test]
fn plan_unwind_basic() {
    // UNWIND [1, 2, 3] AS x RETURN x
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            Clause::Unwind(UnwindClause {
                expr: Expr::List(vec![Expr::Integer(1), Expr::Integer(2), Expr::Integer(3)]),
                alias: Identifier::new("x"),
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("x")),
                    alias: None,
                }],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_unwind(&plan));
}

#[test]
fn plan_unwind_after_match() {
    // MATCH (n:Person) UNWIND n.hobbies AS h RETURN n, h
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::Unwind(UnwindClause {
                expr: Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("n"))),
                    "hobbies".into(),
                ),
                alias: Identifier::new("h"),
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![
                    ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    },
                    ReturnItem {
                        expr: Expr::Variable(Identifier::new("h")),
                        alias: None,
                    },
                ],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_unwind(&plan));
    assert!(matches!(plan, Operator::ProduceResults { .. }));
}
