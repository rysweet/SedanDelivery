//! Tests for write-clause planning: `DELETE`, `SET`, `REMOVE`, `MERGE`.

use super::*;
use crate::ast::{
    Clause, DeleteClause, Expr, Identifier, Label, MergeClause, NodePattern, PathPattern, Query,
    RemoveClause, RemoveClauseItem, SetClause, SetClauseItem,
};

// ── DELETE tests ──────────────────────────────────────────────────────────

#[test]
fn plan_delete_basic() {
    // MATCH (n:Person) DELETE n
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::Delete(DeleteClause {
                exprs: vec![Expr::Variable(Identifier::new("n"))],
                detach: false,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_delete(&plan));
}

#[test]
fn plan_detach_delete() {
    // MATCH (n:Person) DETACH DELETE n
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::Delete(DeleteClause {
                exprs: vec![Expr::Variable(Identifier::new("n"))],
                detach: true,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_delete(&plan));
    // Verify detach flag is set.
    assert!(plan_delete_is_detach(&plan));
}

#[test]
fn plan_delete_without_match_is_error() {
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![Clause::Delete(DeleteClause {
            exprs: vec![Expr::Variable(Identifier::new("n"))],
            detach: false,
        })],
    };
    assert!(planner.plan(&query).is_err());
}

// ── SET tests ─────────────────────────────────────────────────────────────

#[test]
fn plan_set_property() {
    // MATCH (n:Person) SET n.age = 30
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::Set(SetClause {
                items: vec![SetClauseItem::SetProperty {
                    alias: Identifier::new("n"),
                    prop: "age".into(),
                    value: Expr::Integer(30),
                }],
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_set(&plan));
}

#[test]
fn plan_set_labels() {
    // MATCH (n:Person) SET n:Admin:Active
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::Set(SetClause {
                items: vec![SetClauseItem::SetLabels {
                    alias: Identifier::new("n"),
                    labels: vec![Label::new("Admin"), Label::new("Active")],
                }],
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_set(&plan));
}

#[test]
fn plan_set_map_merge() {
    // MATCH (n:Person) SET n += {age: 30, city: 'NY'}
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::Set(SetClause {
                items: vec![SetClauseItem::SetMap {
                    alias: Identifier::new("n"),
                    map: Expr::MapLiteral(vec![
                        (smol_str::SmolStr::new("age"), Expr::Integer(30)),
                        (smol_str::SmolStr::new("city"), Expr::Str("NY".into())),
                    ]),
                    merge: true,
                }],
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_set(&plan));
}

#[test]
fn plan_set_without_match_is_error() {
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![Clause::Set(SetClause {
            items: vec![SetClauseItem::SetProperty {
                alias: Identifier::new("n"),
                prop: "age".into(),
                value: Expr::Integer(30),
            }],
        })],
    };
    assert!(planner.plan(&query).is_err());
}

// ── REMOVE tests ──────────────────────────────────────────────────────────

#[test]
fn plan_remove_property() {
    // MATCH (n:Person) REMOVE n.age
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::Remove(RemoveClause {
                items: vec![RemoveClauseItem::RemoveProperty {
                    alias: Identifier::new("n"),
                    prop: "age".into(),
                }],
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_remove(&plan));
}

#[test]
fn plan_remove_labels() {
    // MATCH (n:Person) REMOVE n:Admin
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "n"),
            Clause::Remove(RemoveClause {
                items: vec![RemoveClauseItem::RemoveLabels {
                    alias: Identifier::new("n"),
                    labels: vec![Label::new("Admin")],
                }],
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_remove(&plan));
}

#[test]
fn plan_remove_without_match_is_error() {
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![Clause::Remove(RemoveClause {
            items: vec![RemoveClauseItem::RemoveProperty {
                alias: Identifier::new("n"),
                prop: "age".into(),
            }],
        })],
    };
    assert!(planner.plan(&query).is_err());
}

// ── MERGE tests ───────────────────────────────────────────────────────────

#[test]
fn plan_merge_basic() {
    // MERGE (n:Person)
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![Clause::Merge(MergeClause {
            pattern: PathPattern {
                variable: None,
                start: NodePattern {
                    variable: Some(Identifier::new("n")),
                    labels: vec![Label::new("Person")],
                    properties: None,
                    where_clause: None,
                },
                steps: vec![],
                shortest_path: None,
            },
            on_create: vec![],
            on_match: vec![],
        })],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_merge(&plan));
}

#[test]
fn plan_merge_with_on_create_on_match() {
    // MERGE (n:Person)
    //   ON CREATE SET n.created = true
    //   ON MATCH SET n.accessed = true
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![Clause::Merge(MergeClause {
            pattern: PathPattern {
                variable: None,
                start: NodePattern {
                    variable: Some(Identifier::new("n")),
                    labels: vec![Label::new("Person")],
                    properties: None,
                    where_clause: None,
                },
                steps: vec![],
                shortest_path: None,
            },
            on_create: vec![SetClauseItem::SetProperty {
                alias: Identifier::new("n"),
                prop: "created".into(),
                value: Expr::Bool(true),
            }],
            on_match: vec![SetClauseItem::SetProperty {
                alias: Identifier::new("n"),
                prop: "accessed".into(),
                value: Expr::Bool(true),
            }],
        })],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_merge(&plan));
    // Verify merge has ON CREATE and ON MATCH items.
    assert!(plan_merge_has_actions(&plan));
}

#[test]
fn plan_merge_after_match() {
    // MATCH (a:Person) MERGE (n:City)
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            make_match_clause("Person", "a"),
            Clause::Merge(MergeClause {
                pattern: PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("n")),
                        labels: vec![Label::new("City")],
                        properties: None,
                        where_clause: None,
                    },
                    steps: vec![],
                    shortest_path: None,
                },
                on_create: vec![],
                on_match: vec![],
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_merge(&plan));
}
