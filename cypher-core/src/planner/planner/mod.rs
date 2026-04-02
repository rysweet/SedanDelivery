//! Query planner — translates a parsed [`Query`] AST into a physical [`Operator`] tree.
//!
//! ## Strategy
//!
//! The planner walks the clause list left-to-right:
//! 1. `MATCH` -> `NodeScan` (or `NodeScan + Expand` for multi-hop patterns) + optional `Filter`.
//! 2. `WHERE` (standalone clause) -> `Filter` wrapping current pipeline.
//! 3. `CREATE` -> `Create` operator, optionally wrapping a `MATCH` pipeline.
//! 4. `RETURN` -> `ProduceResults` wrapping `Project` or `Aggregate`.
//!    - `DISTINCT` -> `Distinct` wrapper.
//!    - `ORDER BY` -> `Sort` wrapper.
//!    - `SKIP` / `LIMIT` -> `Skip` / `Limit` wrappers.
//! 5. `DELETE` / `DETACH DELETE` -> `Delete` operator wrapping the current pipeline.
//! 6. `WITH` -> mid-query projection/aggregation with optional `Filter`, `Sort`, `Skip`, `Limit`.
//! 7. `UNWIND` -> `Unwind` operator for list expansion.
//! 8. `SET` -> `Set` operator for property/label mutation.
//! 9. `REMOVE` -> `Remove` operator for property/label removal.
//! 10. `MERGE` -> `Merge` operator with `ON CREATE SET` / `ON MATCH SET` actions.
//! 11. `OPTIONAL MATCH` -> `LeftOuterApply` wrapping the match pipeline.

pub(crate) mod helpers;
mod match_planner;
mod mutation_planner;
mod result_planner;

use std::collections::HashMap;

use crate::ast::{Clause, Query, ReturnClause};
use crate::error::{GqlError, PlanError};
use crate::planner::Operator;
use crate::procedure::ProcedureRegistry;
use crate::storage::GraphStore;
use crate::types::Value;

use helpers::{empty_params, empty_registry};

// ── Planner ───────────────────────────────────────────────────────────────────

/// Translates a [`Query`] AST into a physical [`Operator`] tree.
pub struct QueryPlanner<'s> {
    pub(crate) store: &'s dyn GraphStore,
    pub(crate) graph: &'s str,
    pub(crate) registry: &'s ProcedureRegistry,
    pub(crate) parameters: &'s HashMap<String, Value>,
}

impl<'s> QueryPlanner<'s> {
    /// Create a new planner backed by `store` targeting `graph`.
    pub fn new(store: &'s dyn GraphStore, graph: &'s str) -> Self {
        Self {
            store,
            graph,
            registry: empty_registry(),
            parameters: empty_params(),
        }
    }

    /// Create a planner with a procedure registry and query parameters.
    pub fn with_context(
        store: &'s dyn GraphStore,
        graph: &'s str,
        registry: &'s ProcedureRegistry,
        parameters: &'s HashMap<String, Value>,
    ) -> Self {
        Self {
            store,
            graph,
            registry,
            parameters,
        }
    }

    /// Plan a query: produce the root [`Operator`] for execution.
    pub fn plan(&self, query: &Query) -> Result<Operator, GqlError> {
        // Run compile-time validation before planning.
        crate::planner::validator::validate_query(query)?;
        self.plan_unchecked(query)
    }

    /// Plan a query without running compile-time validation.
    /// Used for inner EXISTS subqueries that reference outer-scope variables.
    pub fn plan_unchecked(&self, query: &Query) -> Result<Operator, GqlError> {
        let mut pipeline: Option<Operator> = None;
        let mut return_clause: Option<&ReturnClause> = None;

        for (idx, clause) in query.clauses.iter().enumerate() {
            match clause {
                Clause::Match(mc) => {
                    let op = self.plan_match(mc)?;
                    pipeline = Some(match pipeline {
                        None => op,
                        Some(prev) => {
                            // Chain: apply operator on top of previous pipeline.
                            // Multiple MATCH clauses (whether single- or multi-pattern)
                            // are joined via Apply (cross-product semantics).
                            Operator::Apply {
                                outer: Box::new(prev),
                                inner: Box::new(op),
                            }
                        }
                    });
                }

                Clause::OptionalMatch(mc) => {
                    let match_op = self.plan_match(mc)?;
                    // When no prior pipeline exists, use SingleRow so that
                    // OPTIONAL MATCH still produces one row with NULLs even
                    // on an empty graph.
                    let preserved = pipeline.unwrap_or(Operator::SingleRow);
                    pipeline = Some(Operator::LeftOuterApply {
                        preserved: Box::new(preserved),
                        optional_pattern: Box::new(match_op),
                    });
                }

                Clause::Where(pred) => {
                    let child = pipeline.ok_or_else(|| {
                        GqlError::Plan(PlanError::SemanticError {
                            message: "WHERE clause requires a preceding MATCH or WITH clause"
                                .to_string(),
                        })
                    })?;
                    pipeline = Some(Operator::Filter {
                        predicate: pred.clone(),
                        child: Box::new(child),
                    });
                }

                Clause::Return(rc) => {
                    return_clause = Some(rc);
                }

                Clause::Create(cc) => {
                    let create_op = self.plan_create(cc, pipeline.take())?;
                    pipeline = Some(create_op);
                }

                Clause::Delete(dc) => {
                    let delete_op = self.plan_delete(dc, pipeline.take())?;
                    pipeline = Some(delete_op);
                }

                Clause::With(wc) => {
                    let with_op = self.plan_with(wc, pipeline.take())?;
                    pipeline = Some(with_op);
                }

                Clause::Unwind(uc) => {
                    let unwind_op = self.plan_unwind(uc, pipeline.take())?;
                    pipeline = Some(unwind_op);
                }

                Clause::Set(sc) => {
                    let set_op = self.plan_set(sc, pipeline.take())?;
                    pipeline = Some(set_op);
                }

                Clause::Remove(rc) => {
                    let remove_op = self.plan_remove(rc, pipeline.take())?;
                    pipeline = Some(remove_op);
                }

                Clause::Merge(mc) => {
                    let merge_op = self.plan_merge(mc, pipeline.take())?;
                    pipeline = Some(merge_op);
                }

                Clause::Call(cc) => {
                    let call_op = self.plan_call(cc, pipeline.take(), idx, query)?;
                    pipeline = Some(call_op);
                }

                Clause::Foreach(fc) => {
                    let foreach_op = self.plan_foreach(fc, pipeline.take())?;
                    pipeline = Some(foreach_op);
                }

                Clause::CallSubquery { inner } => {
                    // Build a sub-query from the inner clauses and plan it.
                    let sub_query = crate::ast::Query {
                        clauses: inner.clone(),
                        unions: vec![],
                    };
                    let sub_op = self.plan_unchecked(&sub_query)?;
                    pipeline = Some(match pipeline {
                        None => sub_op,
                        Some(prev) => Operator::Apply {
                            outer: Box::new(prev),
                            inner: Box::new(sub_op),
                        },
                    });
                }

                Clause::LoadCsv(lc) => {
                    // LOAD CSV is planned as an Unwind over a function call that
                    // loads the CSV rows at execution time.
                    let load_expr = crate::ast::Expr::FunctionCall {
                        name: smol_str::SmolStr::new("loadcsv"),
                        args: vec![lc.source.clone(), crate::ast::Expr::Bool(lc.with_headers)],
                        distinct: false,
                    };
                    let child_op = pipeline.take().unwrap_or(Operator::SingleRow);
                    pipeline = Some(Operator::Unwind {
                        list_expr: load_expr,
                        alias: lc.alias.clone(),
                        child: Box::new(child_op),
                    });
                }
            }
        }

        // Build the main pipeline result.
        let main_op: Operator = if let Some(rc) = return_clause {
            // RETURN without MATCH: produce exactly one row with no bindings.
            let child = pipeline.unwrap_or(Operator::SingleRow);
            self.plan_return(rc, child)?
        } else if let Some(op) = pipeline {
            // For standalone CALL, derive output columns from the procedure.
            let columns = self.derive_standalone_call_columns(&op);
            Operator::ProduceResults {
                columns,
                child: Box::new(op),
            }
        } else {
            return Err(GqlError::Plan(PlanError::ReturnStarNoVariables));
        };

        // Handle UNION / UNION ALL parts.
        if query.unions.is_empty() {
            return Ok(main_op);
        }

        let mut result = main_op;
        for (all, part_clauses) in &query.unions {
            let part_query = Query {
                clauses: part_clauses.clone(),
                unions: Vec::new(),
            };
            let right = self.plan(&part_query)?;
            result = Operator::Union {
                left: Box::new(result),
                right: Box::new(right),
                all: *all,
            };
        }
        Ok(result)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{
        BinaryOp, DeleteClause, Direction, Expr, Identifier, Label, MatchClause, MergeClause,
        NodePattern, PathPattern, PathStep, Properties, Query, RelPattern, RemoveClause,
        RemoveClauseItem, ReturnClause, ReturnItem, SetClause, SetClauseItem, SortItem,
        UnwindClause, WithClause,
    };
    use crate::storage::InMemoryGraphStore;
    use indexmap::IndexMap;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    fn make_simple_match_query(label: &str, alias: &str) -> Query {
        Query {
            unions: Vec::new(),
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new(alias)),
                            labels: vec![Label::new(label)],
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
                        expr: Expr::Variable(Identifier::new(alias)),
                        alias: None,
                    }],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        }
    }

    /// Build a simple MATCH (n:Label) pipeline (no RETURN) for use in write-clause tests.
    fn make_match_clause(label: &str, alias: &str) -> Clause {
        Clause::Match(MatchClause {
            patterns: vec![PathPattern {
                variable: None,
                start: NodePattern {
                    variable: Some(Identifier::new(alias)),
                    labels: vec![Label::new(label)],
                    properties: None,
                    where_clause: None,
                },
                steps: vec![],
                shortest_path: None,
            }],
            where_clause: None,
        })
    }

    #[test]
    fn plan_simple_match_return() {
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = make_simple_match_query("Person", "p");
        let plan = planner.plan(&query).unwrap();
        // Root must be ProduceResults.
        assert!(matches!(plan, Operator::ProduceResults { .. }));
    }

    #[test]
    fn plan_match_with_where() {
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
                    where_clause: Some(Expr::BinaryOp {
                        op: crate::ast::BinaryOp::Gt,
                        left: Box::new(Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "age".into(),
                        )),
                        right: Box::new(Expr::Integer(18)),
                    }),
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
        // Should have a Filter in the tree.
        assert!(plan_contains_filter(&plan));
    }

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

    // ── WITH tests ────────────────────────────────────────────────────────────

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

    // ── UNWIND tests ──────────────────────────────────────────────────────────

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

    // ── OPTIONAL MATCH tests ──────────────────────────────────────────────────

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

    // ── Multi-pattern MATCH tests ─────────────────────────────────────────────

    #[test]
    fn plan_multi_pattern_match() {
        // MATCH (a:Person), (b:Movie) RETURN a, b
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query {
            unions: Vec::new(),
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![
                        PathPattern {
                            variable: None,
                            start: NodePattern {
                                variable: Some(Identifier::new("a")),
                                labels: vec![Label::new("Person")],
                                properties: None,
                                where_clause: None,
                            },
                            steps: vec![],
                            shortest_path: None,
                        },
                        PathPattern {
                            variable: None,
                            start: NodePattern {
                                variable: Some(Identifier::new("b")),
                                labels: vec![Label::new("Movie")],
                                properties: None,
                                where_clause: None,
                            },
                            steps: vec![],
                            shortest_path: None,
                        },
                    ],
                    where_clause: None,
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("a")),
                            alias: None,
                        },
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("b")),
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
        // Multi-pattern MATCH produces an Apply (cross-product) in the tree.
        assert!(plan_contains_apply(&plan));
    }

    #[test]
    fn plan_multi_pattern_match_with_where() {
        // MATCH (a:Person), (b:Movie) WHERE a.name = 'Alice' RETURN a, b
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query {
            unions: Vec::new(),
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![
                        PathPattern {
                            variable: None,
                            start: NodePattern {
                                variable: Some(Identifier::new("a")),
                                labels: vec![Label::new("Person")],
                                properties: None,
                                where_clause: None,
                            },
                            steps: vec![],
                            shortest_path: None,
                        },
                        PathPattern {
                            variable: None,
                            start: NodePattern {
                                variable: Some(Identifier::new("b")),
                                labels: vec![Label::new("Movie")],
                                properties: None,
                                where_clause: None,
                            },
                            steps: vec![],
                            shortest_path: None,
                        },
                    ],
                    where_clause: Some(Expr::BinaryOp {
                        op: BinaryOp::Eq,
                        left: Box::new(Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("a"))),
                            "name".into(),
                        )),
                        right: Box::new(Expr::Str("Alice".into())),
                    }),
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("a")),
                            alias: None,
                        },
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("b")),
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
        assert!(plan_contains_apply(&plan));
        assert!(plan_contains_filter(&plan));
    }

    // ── Index scan tests ──────────────────────────────────────────────────────

    #[test]
    fn plan_node_scan_upgrades_to_index_scan_when_index_exists() {
        // Given a store with an index on Person.name
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store.create_index("g", "Person", "name").unwrap();

        let planner = QueryPlanner::new(&store, "g");
        // MATCH (n:Person {name: 'Alice'}) RETURN n
        let query = Query {
            unions: Vec::new(),
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("n")),
                            labels: vec![Label::new("Person")],
                            properties: Some(Properties::Map(vec![(
                                smol_str::SmolStr::new("name"),
                                Expr::Str("Alice".into()),
                            )])),
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
        assert!(
            plan_contains_index_scan(&plan),
            "Expected IndexScan in plan when index exists for Person.name, got: {plan:?}"
        );
    }

    #[test]
    fn plan_no_index_uses_node_scan_not_index_scan() {
        // Without a declared index the planner must NOT emit IndexScan —
        // it falls back to an honest NodeScan + Filter instead.
        let store = make_store(); // no indexes
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
                            properties: Some(Properties::Map(vec![(
                                smol_str::SmolStr::new("name"),
                                Expr::Str("Alice".into()),
                            )])),
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
        assert!(
            !plan_contains_index_scan(&plan),
            "Without a declared index, planner should use NodeScan not IndexScan, got: {plan:?}"
        );
    }

    // ── Operator tree search helpers ──────────────────────────────────────────

    fn plan_contains_filter(op: &Operator) -> bool {
        match op {
            Operator::Filter { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_filter(c)),
        }
    }

    fn plan_contains_limit(op: &Operator) -> bool {
        match op {
            Operator::Limit { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_limit(c)),
        }
    }

    fn plan_contains_skip(op: &Operator) -> bool {
        match op {
            Operator::Skip { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_skip(c)),
        }
    }

    fn plan_contains_sort(op: &Operator) -> bool {
        match op {
            Operator::Sort { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_sort(c)),
        }
    }

    fn plan_contains_delete(op: &Operator) -> bool {
        match op {
            Operator::Delete { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_delete(c)),
        }
    }

    fn plan_delete_is_detach(op: &Operator) -> bool {
        match op {
            Operator::Delete { detach, .. } => *detach,
            _ => children_of(op).iter().any(|c| plan_delete_is_detach(c)),
        }
    }

    fn plan_contains_project(op: &Operator) -> bool {
        match op {
            Operator::Project { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_project(c)),
        }
    }

    fn plan_contains_aggregate(op: &Operator) -> bool {
        match op {
            Operator::Aggregate { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_aggregate(c)),
        }
    }

    fn plan_contains_unwind(op: &Operator) -> bool {
        match op {
            Operator::Unwind { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_unwind(c)),
        }
    }

    fn plan_contains_set(op: &Operator) -> bool {
        match op {
            Operator::Set { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_set(c)),
        }
    }

    fn plan_contains_remove(op: &Operator) -> bool {
        match op {
            Operator::Remove { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_remove(c)),
        }
    }

    fn plan_contains_merge(op: &Operator) -> bool {
        match op {
            Operator::Merge { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_merge(c)),
        }
    }

    fn plan_merge_has_actions(op: &Operator) -> bool {
        match op {
            Operator::Merge {
                on_create,
                on_match,
                ..
            } => !on_create.is_empty() && !on_match.is_empty(),
            _ => children_of(op).iter().any(|c| plan_merge_has_actions(c)),
        }
    }

    fn plan_contains_left_outer_apply(op: &Operator) -> bool {
        match op {
            Operator::LeftOuterApply { .. } => true,
            _ => children_of(op)
                .iter()
                .any(|c| plan_contains_left_outer_apply(c)),
        }
    }

    fn plan_contains_apply(op: &Operator) -> bool {
        match op {
            Operator::Apply { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_apply(c)),
        }
    }

    fn plan_contains_distinct(op: &Operator) -> bool {
        match op {
            Operator::Distinct { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_distinct(c)),
        }
    }

    fn plan_contains_index_scan(op: &Operator) -> bool {
        match op {
            Operator::IndexScan { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_index_scan(c)),
        }
    }

    #[test]
    fn index_scan_end_to_end_plan_and_execute() {
        use crate::executor::budget::ExecutionBudget;
        use crate::executor::engine::QueryExecutor;
        use crate::types::Value;

        // Set up store with an index and data.
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store.create_index("g", "Person", "name").unwrap();

        let p1 = IndexMap::from([("name".to_string(), Value::Str("Alice".into()))]);
        let alice = store
            .create_node("g", vec!["Person".to_string()], p1)
            .unwrap();
        let p2 = IndexMap::from([("name".to_string(), Value::Str("Bob".into()))]);
        let _bob = store
            .create_node("g", vec!["Person".to_string()], p2)
            .unwrap();

        // Plan: MATCH (n:Person {name: 'Alice'}) RETURN n
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
                            properties: Some(Properties::Map(vec![(
                                smol_str::SmolStr::new("name"),
                                Expr::Str("Alice".into()),
                            )])),
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

        // Verify planner chose IndexScan.
        assert!(
            plan_contains_index_scan(&plan),
            "Planner should choose IndexScan when index exists, got: {plan:?}"
        );

        // Execute the plan and verify correct results.
        let executor = QueryExecutor::new(&store);
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1, "Should find exactly one Alice");
        assert_eq!(
            result.rows[0].get("n"),
            Some(&Value::Node(alice)),
            "Should return Alice's node"
        );
    }

    /// Extract all child operators from an operator for recursive tree searches.
    fn children_of(op: &Operator) -> Vec<&Operator> {
        match op {
            Operator::SingleRow
            | Operator::NodeScan { .. }
            | Operator::IndexScan { .. }
            | Operator::NodeByIdSeek { .. }
            | Operator::ShortestPath { .. } => vec![],

            Operator::Filter { child, .. }
            | Operator::Project { child, .. }
            | Operator::Aggregate { child, .. }
            | Operator::Distinct { child, .. }
            | Operator::Sort { child, .. }
            | Operator::Limit { child, .. }
            | Operator::Skip { child, .. }
            | Operator::Unwind { child, .. }
            | Operator::Set { child, .. }
            | Operator::Remove { child, .. }
            | Operator::Delete { child, .. }
            | Operator::ProduceResults { child, .. }
            | Operator::Expand { child, .. }
            | Operator::VarLenExpand { child, .. }
            | Operator::AssemblePath { child, .. } => vec![child],

            Operator::Apply { outer, inner }
            | Operator::LeftOuterApply {
                preserved: outer,
                optional_pattern: inner,
            } => vec![outer, inner],

            Operator::Union { left, right, .. } => vec![left, right],

            Operator::Create { child, .. } => match child {
                Some(c) => vec![c],
                None => vec![],
            },

            Operator::Merge { child, .. } => match child {
                Some(c) => vec![c],
                None => vec![],
            },

            Operator::Call { child, .. } => match child {
                Some(c) => vec![c],
                None => vec![],
            },
            Operator::MergeRelationship { child, .. }
            | Operator::GremlinSideEffect { child, .. }
            | Operator::GremlinAggregate { child, .. }
            | Operator::GremlinPath { child, .. }
            | Operator::GremlinTree { child, .. }
            | Operator::Cap { child, .. }
            | Operator::GremlinBarrier { child, .. } => vec![child],
            Operator::ForEach { body, child, .. } | Operator::Repeat { body, child, .. } => {
                vec![child, body]
            }
            Operator::Choose {
                predicate,
                if_true,
                if_false,
                child,
                ..
            } => {
                let mut v = vec![child.as_ref(), predicate.as_ref(), if_true.as_ref()];
                if let Some(f) = if_false {
                    v.push(f.as_ref());
                }
                v
            }
            Operator::Coalesce {
                alternatives,
                child,
            } => {
                let mut v = vec![child.as_ref()];
                v.extend(alternatives.iter());
                v
            }
            Operator::GremlinMatch {
                patterns, child, ..
            } => {
                let mut v = vec![child.as_ref()];
                v.extend(patterns.iter());
                v
            }
            Operator::ChooseMap {
                selector,
                options,
                child,
                ..
            } => {
                let mut v = vec![child.as_ref(), selector.as_ref()];
                for (_, branch, _) in options {
                    v.push(branch.as_ref());
                }
                v
            }
            Operator::GremlinSelect { child, .. }
            | Operator::GremlinGroup { child, .. }
            | Operator::GremlinMapSlice { child, .. }
            | Operator::GremlinSimplePath { child, .. }
            | Operator::GremlinCyclicPath { child, .. }
            | Operator::GremlinIndex { child, .. } => {
                vec![child]
            }
            Operator::GremlinProject { entries, child, .. } => {
                let mut v = vec![child.as_ref()];
                for (_, op, _, _) in entries {
                    v.push(op);
                }
                v
            }
            Operator::GremlinSample { child, .. } => {
                vec![child.as_ref()]
            }
        }
    }
}
