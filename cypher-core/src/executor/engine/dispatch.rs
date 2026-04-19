//! Plan-dispatch: routes each [`Operator`] kind to the appropriate
//! `exec_*` helper implemented in sibling modules.

use crate::error::GqlError;
use crate::planner::Operator;

use super::types::{ExecContext, Row};
use super::QueryExecutor;

impl<'a> QueryExecutor<'a> {
    /// Recursively execute an operator, returning its output rows.
    ///
    /// When profiling is enabled on the budget, records the number of rows
    /// produced by each operator keyed by operator pointer address.
    #[allow(clippy::only_used_in_recursion)]
    pub(crate) fn exec_operator(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        cx.budget.check_or_err()?;

        let rows = match op {
            Operator::SingleRow => self.exec_single_row(cx, op),
            Operator::NodeScan { .. } => self.exec_node_scan(cx, op),
            Operator::Filter { .. } => self.exec_filter(cx, op),
            Operator::Expand { .. } => self.exec_expand(cx, op),
            Operator::Project { .. } => self.exec_project(cx, op),
            Operator::Distinct { .. } => self.exec_distinct(cx, op),
            Operator::Sort { .. } => self.exec_sort(cx, op),
            Operator::Limit { .. } => self.exec_limit(cx, op),
            Operator::Skip { .. } => self.exec_skip(cx, op),
            Operator::Aggregate { .. } => self.exec_aggregate(cx, op),
            Operator::Create { .. } => self.exec_create(cx, op),
            Operator::AssemblePath { .. } => self.exec_assemble_path(cx, op),
            Operator::Call { .. } => self.exec_call(cx, op),
            Operator::ProduceResults { .. } => self.exec_produce_results(cx, op),
            Operator::VarLenExpand { .. } => self.exec_var_len_expand(cx, op),
            Operator::Set { .. } => self.exec_set(cx, op),
            Operator::Remove { .. } => self.exec_remove(cx, op),
            Operator::Delete { .. } => self.exec_delete(cx, op),
            Operator::Merge { .. } => self.exec_merge(cx, op),
            Operator::MergeRelationship { .. } => self.exec_merge_rel(cx, op),
            Operator::Unwind { .. } => self.exec_unwind(cx, op),
            Operator::Union { .. } => self.exec_union(cx, op),
            Operator::LeftOuterApply { .. } => self.exec_left_outer_apply(cx, op),
            Operator::Apply { .. } => self.exec_apply(cx, op),
            Operator::IndexScan { .. } => self.exec_index_scan(cx, op),
            Operator::NodeByIdSeek { .. } => self.exec_node_by_id_seek(cx, op),
            Operator::ShortestPath { .. } => self.exec_shortest_path(cx, op),
            Operator::ForEach { .. } => self.exec_foreach(cx, op),
            Operator::Repeat { .. } => self.exec_repeat(cx, op),
            Operator::Choose { .. } => self.exec_choose(cx, op),
            Operator::ChooseMap { .. } => self.exec_choose_map(cx, op),
            Operator::Coalesce { .. } => self.exec_coalesce(cx, op),
            Operator::GremlinSideEffect { .. } => self.exec_gremlin_side_effect(cx, op),
            Operator::Cap { .. } => self.exec_cap(cx, op),
            Operator::GremlinAggregate { .. } => self.exec_gremlin_aggregate(cx, op),
            Operator::GremlinPath { .. } => self.exec_gremlin_path(cx, op),
            Operator::GremlinTree { .. } => self.exec_gremlin_tree(cx, op),
            Operator::GremlinMatch { .. } => self.exec_gremlin_match(cx, op),
            Operator::GremlinSelect { .. } => self.exec_gremlin_select(cx, op),
            Operator::GremlinGroup { .. } => self.exec_gremlin_group(cx, op),
            Operator::GremlinSample { .. } => self.exec_gremlin_sample(cx, op),
            Operator::GremlinMapSlice { .. } => self.exec_gremlin_map_slice(cx, op),
            Operator::GremlinSimplePath { .. } => self.exec_gremlin_simple_path(cx, op),
            Operator::GremlinCyclicPath { .. } => self.exec_gremlin_cyclic_path(cx, op),
            Operator::GremlinIndex { .. } => self.exec_gremlin_index(cx, op),
            Operator::GremlinProject { .. } => self.exec_gremlin_project(cx, op),
            Operator::GremlinBarrier { .. } => self.exec_gremlin_barrier(cx, op),
        }?;

        // Record per-operator row counts when profiling is enabled.
        if cx.budget.is_profiling() {
            let op_addr = op as *const Operator as usize;
            cx.budget.record_operator_rows(op_addr, rows.len());
        }

        Ok(rows)
    }
}
