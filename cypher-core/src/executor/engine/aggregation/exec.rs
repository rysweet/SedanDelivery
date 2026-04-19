//! `QueryExecutor::exec_aggregate` — the operator entry point that drives
//! grouping plus per-row accumulation into [`AggState`].

use std::collections::HashMap;

use crate::ast::Expr;
use crate::error::GqlError;
use crate::planner::{AggFunction, Operator};
use crate::types::Value;

use super::super::helpers::{expr_to_column_name, inject_parameters};
use super::super::{ExecContext, QueryExecutor, Row};
use super::group_key::{group_key_hash, group_key_string, GroupBucket};
use super::state::AggState;

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_aggregate(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Aggregate {
            keys,
            aggregations,
            child,
        } = op
        else {
            debug_assert!(false, "expected Aggregate operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Aggregate operator".into(),
                },
            ));
        };
        let input = self.exec_operator(cx, child)?;
        cx.budget.check_or_err()?;

        // Group rows by the key expression values.
        // We use a Vec-of-(key_row, agg_state) for ordered output and a
        // hash-first index for O(1) group lookup.
        //
        // Fast path (no hash collisions): only u64 hashes are stored — zero
        // String allocations.  On collision we fall back to canonical_key()
        // comparison, matching the RowDedupSet pattern.
        let mut groups: Vec<(Vec<Value>, Vec<AggState>)> = Vec::new();
        let mut group_index: HashMap<u64, GroupBucket> = HashMap::new();

        for row in &input {
            let ctx = self.build_filter_ctx(cx.graph, row)?;
            let group_key: Vec<Value> = keys
                .iter()
                .map(|expr| self.eval_with_storage(cx.graph, expr, row, &ctx))
                .collect::<Result<Vec<_>, GqlError>>()?;

            // Compute a u64 hash from the group key values via canonical_hash.
            let h = group_key_hash(&group_key);

            // Find existing group or create new — O(1) via the hash index.
            // canonical_hash() ensures equal values (e.g. -0.0/0.0, maps with
            // different insertion order, same-instant DateTimes in different
            // time zones) produce the same hash.
            let idx = match group_index.entry(h) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    // Fast path: hash never seen — zero String allocations.
                    let i = groups.len();
                    let new_states = aggregations.iter().map(|_| AggState::default()).collect();
                    groups.push((group_key, new_states));
                    e.insert(GroupBucket::Single(i));
                    i
                }
                std::collections::hash_map::Entry::Occupied(mut e) => {
                    let key_str = group_key_string(&group_key);
                    match e.get_mut() {
                        GroupBucket::Single(first_idx) => {
                            let first_key_str = group_key_string(&groups[*first_idx].0);
                            if first_key_str == key_str {
                                // Same group
                                let i = *first_idx;
                                *e.get_mut() = GroupBucket::Resolved(vec![(first_key_str, i)]);
                                i
                            } else {
                                // True collision — different groups share the same hash
                                let i = groups.len();
                                let new_states =
                                    aggregations.iter().map(|_| AggState::default()).collect();
                                groups.push((group_key, new_states));
                                let fi = *first_idx;
                                *e.get_mut() =
                                    GroupBucket::Resolved(vec![(first_key_str, fi), (key_str, i)]);
                                i
                            }
                        }
                        GroupBucket::Resolved(entries) => {
                            if let Some(&(_, i)) = entries.iter().find(|(k, _)| *k == key_str) {
                                i
                            } else {
                                let i = groups.len();
                                let new_states =
                                    aggregations.iter().map(|_| AggState::default()).collect();
                                groups.push((group_key, new_states));
                                entries.push((key_str, i));
                                i
                            }
                        }
                    }
                }
            };

            // Accumulate into each aggregation state.
            for (agg_idx, (func, arg_expr, second_arg_expr, distinct, _alias)) in
                aggregations.iter().enumerate()
            {
                // count(*) — the `*` argument is a sentinel meaning "count rows".
                // Use a non-null placeholder so the accumulator counts every row.
                let arg_val = if matches!(arg_expr, Expr::Variable(ref id) if id.0 == "*") {
                    Value::Bool(true)
                } else {
                    self.eval_with_storage(cx.graph, arg_expr, row, &ctx)?
                };
                let second_val = second_arg_expr
                    .as_ref()
                    .map(|e| self.eval_with_storage(cx.graph, e, row, &ctx))
                    .transpose()?;
                groups[idx].1[agg_idx].accumulate(func, arg_val, second_val, *distinct);
                // __gremlin_bulk: when the row carries a bulk count > 1 and
                // we're doing Count, add (bulk - 1) extra counts.
                if matches!(func, AggFunction::Count) {
                    if let Some(Value::Int(bulk)) = row.get("__gremlin_bulk") {
                        if *bulk > 1 {
                            for _ in 1..*bulk {
                                groups[idx].1[agg_idx].accumulate(
                                    func,
                                    Value::Bool(true),
                                    None,
                                    *distinct,
                                );
                            }
                        }
                    }
                }
            }
        }

        // Handle empty input with no-key aggregation (e.g. COUNT(*) on empty).
        if groups.is_empty() && keys.is_empty() {
            let new_states: Vec<AggState> =
                aggregations.iter().map(|_| AggState::default()).collect();
            groups.push((vec![], new_states));
        }

        // Build output rows.
        let mut out = Vec::with_capacity(groups.len());
        for (key_vals, agg_states) in groups {
            cx.budget.check_or_err()?;
            let mut row = Row::new();

            // Add group key columns — derive names from key expressions.
            for (expr, val) in keys.iter().zip(key_vals) {
                row.insert(expr_to_column_name(expr), val);
            }

            // Add aggregation result columns.
            for ((func, _, _, _, alias), state) in aggregations.iter().zip(&agg_states) {
                row.insert(alias.0.clone(), state.finalize(func)?);
            }
            // Preserve query parameters so downstream operators (e.g. Delete,
            // Filter) can still access $param bindings after aggregation.
            inject_parameters(&mut row, &self.parameters);
            cx.budget.record_row();
            out.push(row);
        }
        Ok(out)
    }
}
