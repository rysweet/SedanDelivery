//! SET and REMOVE operator implementations.
//!
//! Submodules group logic by SET-clause variant:
//! - [`property`] — `SET n.prop = v` and `REMOVE n.prop`
//! - [`map`]      — `SET n = {..}` (replace) and `SET n += {..}` (merge)
//! - [`labels`]   — `SET n:Label`, `SET n:$(expr)`, and `REMOVE n:Label`

use crate::error::{ExecutionError, GqlError};
use crate::planner::{Operator, RemoveItem, SetItem};

use crate::executor::engine::{ExecContext, MutationStats, QueryExecutor, Row};

mod labels;
mod map;
mod property;

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_set(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Set { items, child } = op else {
            debug_assert!(false, "expected Set operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Set operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;
            let current_row = row;
            for item in items {
                self.apply_set_item(cx.graph, &current_row, item, cx.stats)?;
            }
            cx.budget.record_row();
            out.push(current_row);
        }
        self.invalidate_property_cache();
        Ok(out)
    }

    pub(crate) fn exec_remove(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Remove { items, child } = op else {
            debug_assert!(false, "expected Remove operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Remove operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;
            for item in items {
                match item {
                    RemoveItem::Property { alias, key } => {
                        self.apply_remove_property(cx.graph, &row, alias, key)?;
                    }
                    RemoveItem::Labels { alias, labels } => {
                        self.apply_remove_labels(cx.graph, &row, alias, labels, cx.stats)?;
                    }
                }
            }
            cx.budget.record_row();
            out.push(row);
        }
        self.invalidate_property_cache();
        Ok(out)
    }

    /// Apply a single `SetItem` (SET n.prop = val, SET n = {map}, SET n += {map}, SET n:Label).
    /// Handles both Node and Edge targets for property operations.
    pub(crate) fn apply_set_item(
        &self,
        graph: &str,
        row: &Row,
        item: &SetItem,
        stats: &mut MutationStats,
    ) -> Result<(), GqlError> {
        match item {
            SetItem::Property { alias, key, value } => {
                self.apply_set_property(graph, row, alias, key, value, stats)
            }
            SetItem::Map { alias, map, merge } => {
                self.apply_set_map(graph, row, alias, map, *merge, stats)
            }
            SetItem::Labels { alias, labels } => {
                self.apply_set_labels(graph, row, alias, labels, stats)
            }
            SetItem::DynamicLabels { alias, value } => {
                self.apply_set_dynamic_labels(graph, row, alias, value, stats)
            }
        }
    }
}

#[cfg(test)]
mod tests;
