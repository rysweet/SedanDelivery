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

#[cfg(test)]
mod tests;

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
