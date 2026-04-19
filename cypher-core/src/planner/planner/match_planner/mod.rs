//! MATCH and OPTIONAL MATCH planning — pattern compilation and edge expansion.
//!
//! Decomposed into focused submodules:
//! - [`path_pattern`]: single-pattern compilation into nested NodeScan/Expand
//!   operators with inline filters.
//! - [`shortest_path`]: `shortestPath(...)` / `allShortestPaths(...)` planning.
//!
//! This module retains the top-level entry points (`plan_match`,
//! `plan_exists_subquery`) and the small `plan_node_scan` helper used across
//! sibling planners.

mod path_pattern;
mod shortest_path;

use crate::ast::{Expr, Identifier, Label, MatchClause, PathPattern};
use crate::error::GqlError;
use crate::planner::Operator;

use super::QueryPlanner;

impl<'s> QueryPlanner<'s> {
    // ── MATCH planning ────────────────────────────────────────────────────────

    pub(crate) fn plan_match(&self, mc: &MatchClause) -> Result<Operator, GqlError> {
        if mc.patterns.is_empty() {
            return Err(GqlError::Plan(
                crate::error::PlanError::ReturnStarNoVariables,
            ));
        }

        // Plan each pattern, then join them (multiple patterns = cross-product
        // via Apply for disconnected patterns).
        let mut pattern_ops: Vec<Operator> = mc
            .patterns
            .iter()
            .map(|p| self.plan_path_pattern(p))
            .collect::<Result<Vec<_>, _>>()?;

        let mut op = pattern_ops.remove(0);
        for other in pattern_ops {
            op = Operator::Apply {
                outer: Box::new(op),
                inner: Box::new(other),
            };
        }

        // Append WHERE clause from MatchClause if present.
        if let Some(pred) = &mc.where_clause {
            op = Operator::Filter {
                predicate: pred.clone(),
                child: Box::new(op),
            };
        }

        Ok(op)
    }

    /// Plan an existential subquery: `EXISTS { patterns [WHERE predicate] }`.
    ///
    /// Called by the executor when evaluating `Expr::ExistsSubquery` at runtime.
    /// Returns an operator that produces rows if the patterns match.
    pub fn plan_exists_subquery(
        &self,
        patterns: &[PathPattern],
        where_clause: Option<&Expr>,
    ) -> Result<Operator, GqlError> {
        if patterns.is_empty() {
            return Ok(Operator::SingleRow);
        }
        let mut pattern_ops: Vec<Operator> = patterns
            .iter()
            .map(|p| self.plan_path_pattern(p))
            .collect::<Result<Vec<_>, _>>()?;
        let mut op = pattern_ops.remove(0);
        for other in pattern_ops {
            op = Operator::Apply {
                outer: Box::new(op),
                inner: Box::new(other),
            };
        }
        if let Some(pred) = where_clause {
            op = Operator::Filter {
                predicate: pred.clone(),
                child: Box::new(op),
            };
        }
        Ok(op)
    }

    pub(crate) fn plan_node_scan(&self, alias: Identifier, label: Option<Label>) -> Operator {
        Operator::NodeScan { label, alias }
    }
}
