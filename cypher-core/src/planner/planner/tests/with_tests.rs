//! Tests for `WITH` planning: projection, filter, aggregation, ordering, distinct.

use super::*;
use crate::ast::{
    BinaryOp, Clause, Expr, Identifier, Query, ReturnClause, ReturnItem, SortItem, WithClause,
};

#[test]
fn plan_with_simple_projection() {
    // MATCH (n:Person) WITH n RETURN n
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::With(WithClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("n")),
                    alias: None,
                }],
                where_clause: None,
                order_by: vec![],
                skip: None,
                limit: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("n")),
                    alias: None,
                }],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(matches!(plan, Operator::ProduceResults { .. }));
    assert!(plan_contains_project(&plan));
}

#[test]
fn plan_with_where_filter() {
    // MATCH (n:Person) WITH n WHERE n.age > 25 RETURN n
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::With(WithClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("n")),
                    alias: None,
                }],
                where_clause: Some(Expr::BinaryOp {
                    op: BinaryOp::Gt,
                    left: Box::new(Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("n"))),
                        "age".into(),
                    )),
                    right: Box::new(Expr::Integer(25)),
                }),
                order_by: vec![],
                skip: None,
                limit: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("n")),
                    alias: None,
                }],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_filter(&plan));
    assert!(plan_contains_project(&plan));
}

#[test]
fn plan_with_aggregation() {
    // MATCH (n:Person) WITH n.city AS city, count(n) AS cnt RETURN city, cnt
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::With(WithClause {
                distinct: false,
                items: vec![
                    ReturnItem {
                        expr: Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "city".into(),
                        ),
                        alias: Some(Identifier::new("city")),
                    },
                    ReturnItem {
                        expr: Expr::FunctionCall {
                            name: "count".into(),
                            args: vec![Expr::Variable(Identifier::new("n"))],
                            distinct: false,
                        },
                        alias: Some(Identifier::new("cnt")),
                    },
                ],
                where_clause: None,
                order_by: vec![],
                skip: None,
                limit: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![
                    ReturnItem {
                        expr: Expr::Variable(Identifier::new("city")),
                        alias: None,
                    },
                    ReturnItem {
                        expr: Expr::Variable(Identifier::new("cnt")),
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
    assert!(plan_contains_aggregate(&plan));
}

#[test]
fn plan_with_order_skip_limit() {
    // MATCH (n:Person) WITH n ORDER BY n.name SKIP 1 LIMIT 5 RETURN n
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::With(WithClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("n")),
                    alias: None,
                }],
                where_clause: None,
                order_by: vec![SortItem {
                    expr: Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("n"))),
                        "name".into(),
                    ),
                    ascending: true,
                }],
                skip: Some(Expr::Integer(1)),
                limit: Some(Expr::Integer(5)),
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("n")),
                    alias: None,
                }],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_sort(&plan));
    assert!(plan_contains_skip(&plan));
    assert!(plan_contains_limit(&plan));
}

#[test]
fn plan_with_distinct() {
    // MATCH (n:Person) WITH DISTINCT n.city AS city RETURN city
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::With(WithClause {
                distinct: true,
                items: vec![ReturnItem {
                    expr: Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("n"))),
                        "city".into(),
                    ),
                    alias: Some(Identifier::new("city")),
                }],
                where_clause: None,
                order_by: vec![],
                skip: None,
                limit: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("city")),
                    alias: None,
                }],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_distinct(&plan));
}
