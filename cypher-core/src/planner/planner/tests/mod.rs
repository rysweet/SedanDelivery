//! Unit tests for `QueryPlanner`, organised by clause family.
//!
//! Common helpers (graph store factory, AST builders, plan-tree predicates)
//! live in this module so each submodule can `use super::*`.

use super::*;
use crate::ast::Expr;
use crate::ast::{
    Clause, Identifier, Label, MatchClause, NodePattern, PathPattern, Query, ReturnClause,
    ReturnItem,
};
use crate::storage::InMemoryGraphStore;

mod match_tests;
mod mutation_tests;
mod optional_tests;
mod return_tests;
mod unwind_tests;
mod with_tests;

// ── Fixture builders ──────────────────────────────────────────────────────────

pub(super) fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

pub(super) fn make_simple_match_query(label: &str, alias: &str) -> Query {
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
pub(super) fn make_match_clause(label: &str, alias: &str) -> Clause {
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

// ── Operator tree search helpers ──────────────────────────────────────────────

pub(super) fn plan_contains_filter(op: &Operator) -> bool {
    match op {
        Operator::Filter { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_filter(c)),
    }
}

pub(super) fn plan_contains_limit(op: &Operator) -> bool {
    match op {
        Operator::Limit { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_limit(c)),
    }
}

pub(super) fn plan_contains_skip(op: &Operator) -> bool {
    match op {
        Operator::Skip { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_skip(c)),
    }
}

pub(super) fn plan_contains_sort(op: &Operator) -> bool {
    match op {
        Operator::Sort { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_sort(c)),
    }
}

pub(super) fn plan_contains_delete(op: &Operator) -> bool {
    match op {
        Operator::Delete { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_delete(c)),
    }
}

pub(super) fn plan_delete_is_detach(op: &Operator) -> bool {
    match op {
        Operator::Delete { detach, .. } => *detach,
        _ => children_of(op).iter().any(|c| plan_delete_is_detach(c)),
    }
}

pub(super) fn plan_contains_project(op: &Operator) -> bool {
    match op {
        Operator::Project { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_project(c)),
    }
}

pub(super) fn plan_contains_aggregate(op: &Operator) -> bool {
    match op {
        Operator::Aggregate { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_aggregate(c)),
    }
}

pub(super) fn plan_contains_unwind(op: &Operator) -> bool {
    match op {
        Operator::Unwind { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_unwind(c)),
    }
}

pub(super) fn plan_contains_set(op: &Operator) -> bool {
    match op {
        Operator::Set { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_set(c)),
    }
}

pub(super) fn plan_contains_remove(op: &Operator) -> bool {
    match op {
        Operator::Remove { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_remove(c)),
    }
}

pub(super) fn plan_contains_merge(op: &Operator) -> bool {
    match op {
        Operator::Merge { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_merge(c)),
    }
}

pub(super) fn plan_merge_has_actions(op: &Operator) -> bool {
    match op {
        Operator::Merge {
            on_create,
            on_match,
            ..
        } => !on_create.is_empty() && !on_match.is_empty(),
        _ => children_of(op).iter().any(|c| plan_merge_has_actions(c)),
    }
}

pub(super) fn plan_contains_left_outer_apply(op: &Operator) -> bool {
    match op {
        Operator::LeftOuterApply { .. } => true,
        _ => children_of(op)
            .iter()
            .any(|c| plan_contains_left_outer_apply(c)),
    }
}

pub(super) fn plan_contains_apply(op: &Operator) -> bool {
    match op {
        Operator::Apply { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_apply(c)),
    }
}

pub(super) fn plan_contains_distinct(op: &Operator) -> bool {
    match op {
        Operator::Distinct { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_distinct(c)),
    }
}

pub(super) fn plan_contains_index_scan(op: &Operator) -> bool {
    match op {
        Operator::IndexScan { .. } => true,
        _ => children_of(op).iter().any(|c| plan_contains_index_scan(c)),
    }
}

/// Extract all child operators from an operator for recursive tree searches.
pub(super) fn children_of(op: &Operator) -> Vec<&Operator> {
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
