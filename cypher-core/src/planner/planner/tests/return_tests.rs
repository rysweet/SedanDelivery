//! Tests for `RETURN` planning: `LIMIT`, `SKIP`, and `ORDER BY`.

use super::*;
use crate::ast::{
    Clause, Expr, Identifier, Label, MatchClause, NodePattern, PathPattern, Query, ReturnClause,
    ReturnItem, SortItem,
};

#[test]
fn plan_return_with_limit_skip() {
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("n")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                        where_clause: None,
                    },
                    steps: vec![],
                    shortest_path: None,
                }],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("n")),
                    alias: None,
                }],
                order_by: vec![],
                skip: Some(Expr::Integer(2)),
                limit: Some(Expr::Integer(5)),
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_limit(&plan));
    assert!(plan_contains_skip(&plan));
}

#[test]
fn plan_return_with_order_by() {
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("n")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                        where_clause: None,
                    },
                    steps: vec![],
                    shortest_path: None,
                }],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("n")),
                    alias: None,
                }],
                order_by: vec![SortItem {
                    expr: Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("n"))),
                        "name".into(),
                    ),
                    ascending: true,
                }],
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_sort(&plan));
}
