//! `Limit` operator with leaf-scan early termination, plus the helpers
//! that decide when early termination is safe.

use crate::ast::Expr;
use crate::error::GqlError;
use crate::planner::Operator;

use super::super::helpers::eval_count_expr;
use super::super::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_limit(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Limit { count, child } = op else {
            debug_assert!(false, "expected Limit operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Limit operator".into(),
                },
            ));
        };

        // Detect gremlin_tail(n): take the last n rows preserving current
        // order, instead of the first n.
        if let Expr::FunctionCall { name, args, .. } = count {
            if name.as_str() == "gremlin_tail" {
                let n = eval_count_expr(&args[0], &self.parameters)?;
                let input = self.exec_operator(cx, child)?;
                let skip = input.len().saturating_sub(n);
                cx.budget.check_or_err()?;
                return Ok(input.into_iter().skip(skip).collect());
            }
        }

        let n = eval_count_expr(count, &self.parameters)?;

        // Early termination optimisation: tell leaf scan operators to stop
        // producing rows once we have enough. This is only safe when:
        // 1. The child subtree contains no mutation operators (Create,
        //    Delete, Set, Remove, Merge) — mutations must execute on ALL
        //    qualifying rows regardless of the result-set LIMIT.
        // 2. The child subtree is a "simple pipeline" — no row-multiplying
        //    operators (Expand, VarLenExpand, Unwind, Apply, LeftOuterApply,
        //    Union) between the Limit and the leaf scan. Multiplying
        //    operators break the 1:1 mapping between leaf rows produced
        //    and output rows, making a leaf-level cap unreliable.
        let can_early_terminate = Self::subtree_is_simple_pipeline(child);
        let prev_cap = if can_early_terminate {
            let needed = n.saturating_add(Self::child_skip_amount(child, &self.parameters));
            Some(cx.budget.set_row_cap(needed))
        } else {
            None
        };
        let input = self.exec_operator(cx, child)?;
        if let Some(prev) = prev_cap {
            cx.budget.restore_row_cap(prev);
        }

        cx.budget.check_or_err()?;
        Ok(input.into_iter().take(n).collect())
    }

    /// If `op` is a `Skip` operator, evaluate and return its skip count.
    /// Otherwise return 0. This is used by `exec_limit` to account for
    /// rows consumed by an immediate Skip child.
    fn child_skip_amount(op: &Operator, params: &Row) -> usize {
        if let Operator::Skip { count, .. } = op {
            eval_count_expr(count, params).unwrap_or(0)
        } else {
            0
        }
    }

    /// Returns `true` if `op` is a "simple pipeline" — a linear chain of
    /// 1:1 operators ending at a single leaf scan. Early termination is
    /// only safe when every row produced by the leaf scan maps to exactly
    /// one output row, so no row-multiplying (Expand, VarLenExpand, Unwind)
    /// or joining (Apply, LeftOuterApply, Union) operators are allowed,
    /// and no mutation operators (Create, Delete, Set, Remove, Merge, Call)
    /// are present.
    fn subtree_is_simple_pipeline(op: &Operator) -> bool {
        match op {
            // Leaf operators — pipeline terminus.
            Operator::SingleRow
            | Operator::NodeScan { .. }
            | Operator::IndexScan { .. }
            | Operator::NodeByIdSeek { .. } => true,

            // 1:1 pass-through operators that don't reduce rows — safe.
            Operator::Project { child, .. }
            | Operator::Limit { child, .. }
            | Operator::Skip { child, .. }
            | Operator::AssemblePath { child, .. }
            | Operator::ProduceResults { child, .. } => Self::subtree_is_simple_pipeline(child),

            // Filter may reject rows, so the leaf-level cap doesn't guarantee
            // enough output rows. Sort/Distinct/Aggregate need ALL input
            // rows to produce correct output. All of these are barriers.
            Operator::Filter { .. }
            | Operator::Sort { .. }
            | Operator::Distinct { .. }
            | Operator::Aggregate { .. } => false,

            // Row-multiplying, joining, or mutating operators — not safe.
            Operator::Expand { .. }
            | Operator::VarLenExpand { .. }
            | Operator::Unwind { .. }
            | Operator::ShortestPath { .. }
            | Operator::Union { .. }
            | Operator::Apply { .. }
            | Operator::LeftOuterApply { .. }
            | Operator::Create { .. }
            | Operator::Delete { .. }
            | Operator::Set { .. }
            | Operator::Remove { .. }
            | Operator::Merge { .. }
            | Operator::MergeRelationship { .. }
            | Operator::Call { .. }
            | Operator::ForEach { .. }
            | Operator::Repeat { .. }
            | Operator::Choose { .. }
            | Operator::Coalesce { .. }
            | Operator::GremlinSideEffect { .. }
            | Operator::GremlinAggregate { .. }
            | Operator::GremlinTree { .. }
            | Operator::GremlinPath { .. }
            | Operator::GremlinMatch { .. }
            | Operator::Cap { .. }
            | Operator::ChooseMap { .. }
            | Operator::GremlinSelect { .. }
            | Operator::GremlinGroup { .. }
            | Operator::GremlinSample { .. }
            | Operator::GremlinMapSlice { .. }
            | Operator::GremlinSimplePath { .. }
            | Operator::GremlinCyclicPath { .. }
            | Operator::GremlinIndex { .. }
            | Operator::GremlinProject { .. }
            | Operator::GremlinBarrier { .. } => false,
        }
    }
}
