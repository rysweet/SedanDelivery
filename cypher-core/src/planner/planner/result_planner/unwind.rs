//! UNWIND clause planning.

use crate::ast::UnwindClause;
use crate::error::GqlError;
use crate::planner::Operator;

use super::super::QueryPlanner;

impl<'s> QueryPlanner<'s> {
    // ── UNWIND planning ───────────────────────────────────────────────────────

    pub(crate) fn plan_unwind(
        &self,
        uc: &UnwindClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        let child = child.unwrap_or(Operator::SingleRow);

        Ok(Operator::Unwind {
            list_expr: uc.expr.clone(),
            alias: uc.alias.clone(),
            child: Box::new(child),
        })
    }
}
