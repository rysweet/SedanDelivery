//! Tests for `OPTIONAL MATCH` planning.

use super::*;
use crate::ast::{
    Clause, Direction, Expr, Identifier, Label, MatchClause, NodePattern, PathPattern, PathStep,
    Query, RelPattern, ReturnClause, ReturnItem,
};

#[test]
fn plan_optional_match_basic() {
    // MATCH (n:Person) OPTIONAL MATCH (n)-[:KNOWS]->(m) RETURN n, m
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::OptionalMatch(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("n")),
                        labels: vec![],
                        properties: None,
                        where_clause: None,
                    },
                    steps: vec![PathStep {
                        rel: RelPattern {
                            variable: None,
                            rel_types: vec![crate::ast::RelType::new("KNOWS")],
                            direction: Direction::Outgoing,
                            range: None,
                            properties: None,
                            where_clause: None,
                        },
                        node: NodePattern {
                            variable: Some(Identifier::new("m")),
                            labels: vec![],
                            properties: None,
                            where_clause: None,
                        },
                    }],
                    shortest_path: None,
                }],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![
                    ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    },
                    ReturnItem {
                        expr: Expr::Variable(Identifier::new("m")),
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
    assert!(plan_contains_left_outer_apply(&plan));
}

#[test]
fn plan_optional_match_without_prior_match() {
    // OPTIONAL MATCH (n:Person) RETURN n
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            Clause::OptionalMatch(MatchClause {
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
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    // Should still produce a LeftOuterApply even without a prior MATCH.
    assert!(plan_contains_left_outer_apply(&plan));
}
