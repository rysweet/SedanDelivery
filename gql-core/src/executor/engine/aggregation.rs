//! Aggregation operator and state for the query executor.

use std::collections::{HashMap, HashSet};

use crate::ast::Expr;
use crate::error::{ExecutionError, GqlError};
use crate::executor::budget::ExecutionBudget;
use crate::planner::{AggFunction, Operator};
use crate::types::Value;

use super::helpers::{cmp_values, expr_to_column_name, inject_parameters};
use super::{MutationStats, QueryExecutor, Row};

// ── Aggregation state ─────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub(crate) struct AggState {
    count: i64,
    sum: Option<f64>,
    min: Option<Value>,
    max: Option<Value>,
    collected: Vec<Value>,
    seen_distinct: HashSet<String>,
    /// Percentile parameter for percentileDisc/percentileCont (0.0..=1.0).
    percentile: Option<f64>,
}

impl AggState {
    pub(crate) fn accumulate(
        &mut self,
        func: &AggFunction,
        val: Value,
        second_val: Option<Value>,
        distinct: bool,
    ) {
        // For distinct aggregations, skip duplicates.
        // Cap the set size to prevent OOM via hostile queries with millions
        // of distinct values — consistent with max_collect_size guard on Collect.
        if distinct {
            let max_size = crate::config::global().max_collect_size as usize;
            if self.seen_distinct.len() >= max_size {
                return; // silently drop excess — same as Collect overflow
            }
            let key = val.canonical_key();
            if !self.seen_distinct.insert(key) {
                return;
            }
        }

        match func {
            AggFunction::Count => {
                if !val.is_null() {
                    self.count += 1;
                }
            }
            AggFunction::Sum => {
                if let Some(f) = val.as_float() {
                    self.sum = Some(self.sum.unwrap_or(0.0) + f);
                }
            }
            AggFunction::Avg => {
                if let Some(f) = val.as_float() {
                    self.sum = Some(self.sum.unwrap_or(0.0) + f);
                    self.count += 1;
                }
            }
            AggFunction::Min => {
                if !val.is_null() {
                    self.min = Some(match self.min.take() {
                        None => val,
                        Some(current) => {
                            if cmp_values(&val, &current) == std::cmp::Ordering::Less {
                                val
                            } else {
                                current
                            }
                        }
                    });
                }
            }
            AggFunction::Max => {
                if !val.is_null() {
                    self.max = Some(match self.max.take() {
                        None => val,
                        Some(current) => {
                            if cmp_values(&val, &current) == std::cmp::Ordering::Greater {
                                val
                            } else {
                                current
                            }
                        }
                    });
                }
            }
            AggFunction::Collect => {
                let max_collect = crate::config::global().max_collect_size;
                if !val.is_null() && self.collected.len() < max_collect {
                    self.collected.push(val);
                }
            }
            AggFunction::StDev | AggFunction::StDevP => {
                let max_collect = crate::config::global().max_collect_size;
                if !val.is_null() && self.collected.len() < max_collect {
                    if let Some(f) = val.as_float() {
                        self.collected.push(Value::Float(f));
                    }
                }
            }
            AggFunction::PercentileDisc | AggFunction::PercentileCont => {
                let max_collect = crate::config::global().max_collect_size;
                if !val.is_null() && self.collected.len() < max_collect {
                    if let Some(f) = val.as_float() {
                        self.collected.push(Value::Float(f));
                    }
                }
                // Store the percentile parameter (constant across rows, first one wins).
                if self.percentile.is_none() {
                    if let Some(pval) = &second_val {
                        if let Some(f) = pval.as_float() {
                            self.percentile = Some(f);
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn finalize(&self, func: &AggFunction) -> Result<Value, GqlError> {
        match func {
            AggFunction::Count => Ok(Value::Int(self.count)),
            AggFunction::Sum => Ok(self.sum.map(Value::Float).unwrap_or(Value::Int(0))),
            AggFunction::Avg => {
                if self.count == 0 {
                    Ok(Value::Null)
                } else {
                    Ok(Value::Float(self.sum.unwrap_or(0.0) / self.count as f64))
                }
            }
            AggFunction::Min => Ok(self.min.clone().unwrap_or(Value::Null)),
            AggFunction::Max => Ok(self.max.clone().unwrap_or(Value::Null)),
            AggFunction::Collect => Ok(Value::List(self.collected.clone())),
            AggFunction::StDev => {
                let nums: Vec<f64> = self.collected.iter().filter_map(|v| v.as_float()).collect();
                if nums.len() <= 1 {
                    return Ok(Value::Float(0.0));
                }
                let mean = nums.iter().sum::<f64>() / nums.len() as f64;
                let variance =
                    nums.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (nums.len() - 1) as f64;
                Ok(Value::Float(variance.sqrt()))
            }
            AggFunction::StDevP => {
                let nums: Vec<f64> = self.collected.iter().filter_map(|v| v.as_float()).collect();
                if nums.is_empty() {
                    return Ok(Value::Float(0.0));
                }
                let mean = nums.iter().sum::<f64>() / nums.len() as f64;
                let variance =
                    nums.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / nums.len() as f64;
                Ok(Value::Float(variance.sqrt()))
            }
            AggFunction::PercentileDisc => {
                let p = self.percentile.unwrap_or(0.5);
                if !(0.0..=1.0).contains(&p) {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "percentile in range [0.0, 1.0]".to_string(),
                        actual: format!("NumberOutOfRange: {p}"),
                    }));
                }
                let mut nums: Vec<f64> =
                    self.collected.iter().filter_map(|v| v.as_float()).collect();
                if nums.is_empty() {
                    return Ok(Value::Null);
                }
                nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                // Nearest rank method: index = ceil(p * n) - 1 (clamped)
                let n = nums.len();
                let idx = ((p * n as f64).ceil() as usize)
                    .saturating_sub(1)
                    .min(n - 1);
                // Return as integer if all values are whole numbers
                let v = nums[idx];
                if v.fract() == 0.0 && v >= i64::MIN as f64 && v <= i64::MAX as f64 {
                    Ok(Value::Int(v as i64))
                } else {
                    Ok(Value::Float(v))
                }
            }
            AggFunction::PercentileCont => {
                let p = self.percentile.unwrap_or(0.5);
                if !(0.0..=1.0).contains(&p) {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "percentile in range [0.0, 1.0]".to_string(),
                        actual: format!("NumberOutOfRange: {p}"),
                    }));
                }
                let mut nums: Vec<f64> =
                    self.collected.iter().filter_map(|v| v.as_float()).collect();
                if nums.is_empty() {
                    return Ok(Value::Null);
                }
                nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let idx = p * (nums.len() - 1) as f64;
                let lo = idx.floor() as usize;
                let hi = (idx.ceil() as usize).min(nums.len() - 1);
                if lo == hi {
                    Ok(Value::Float(nums[lo]))
                } else {
                    let frac = idx - lo as f64;
                    Ok(Value::Float(nums[lo] * (1.0 - frac) + nums[hi] * frac))
                }
            }
        }
    }
}

// ── QueryExecutor impl for aggregation ────────────────────────────────────────

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_aggregate(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Aggregate {
            keys,
            aggregations,
            child,
        } = op
        else {
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        budget.check_or_err()?;

        // Group rows by the key expression values.
        // We use a Vec-of-(key_row, agg_state) for ordered output and a
        // HashMap<String, usize> index for O(1) group lookup (Value doesn't impl Hash,
        // so we serialize the key via Debug for the index key).
        let mut groups: Vec<(Vec<Value>, Vec<AggState>)> = Vec::new();
        let mut group_index: HashMap<String, usize> = HashMap::new();

        for row in &input {
            let ctx = self.build_filter_ctx(graph, row)?;
            let group_key: Vec<Value> = keys
                .iter()
                .map(|expr| self.eval_with_storage(graph, expr, row, &ctx))
                .collect::<Result<Vec<_>, GqlError>>()?;

            // Find existing group or create new — O(1) via the index.
            // canonical_key() ensures equal values (e.g. -0.0/0.0, maps with
            // different insertion order, same-instant DateTimes in different
            // time zones) produce the same hash key.
            let key_str: String = group_key
                .iter()
                .map(|v| v.canonical_key())
                .collect::<Vec<_>>()
                .join("|");
            let idx = if let Some(&i) = group_index.get(&key_str) {
                i
            } else {
                let i = groups.len();
                let new_states = aggregations.iter().map(|_| AggState::default()).collect();
                groups.push((group_key, new_states));
                group_index.insert(key_str, i);
                i
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
                    self.eval_with_storage(graph, arg_expr, row, &ctx)?
                };
                let second_val = second_arg_expr
                    .as_ref()
                    .map(|e| self.eval_with_storage(graph, e, row, &ctx))
                    .transpose()?;
                groups[idx].1[agg_idx].accumulate(func, arg_val, second_val, *distinct);
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
            budget.check_or_err()?;
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
            budget.record_row();
            out.push(row);
        }
        Ok(out)
    }
}
