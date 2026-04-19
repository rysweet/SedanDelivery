//! `GremlinBarrier` — bulk synchronization with optional sack normalization.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::executor::engine::{ExecContext, QueryExecutor, Row};
use crate::planner::Operator;
use crate::types::Value;

impl<'a> QueryExecutor<'a> {
    /// Execute `GremlinBarrier` — bulk synchronization with optional sack normalization.
    pub(crate) fn exec_gremlin_barrier(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinBarrier {
            child,
            current_alias,
            out_alias,
            norm_sack,
            sack_merge_op,
            with_bulk,
        } = op
        else {
            debug_assert!(false, "expected GremlinBarrier operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinBarrier operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;

        // If no merge and not normSack, pass-through
        if !*norm_sack && sack_merge_op.is_none() {
            let mut out = Vec::with_capacity(input.len());
            for mut row in input {
                if let Some(val) = row.get(current_alias.0.as_str()).cloned() {
                    row.insert(out_alias.0.clone(), val);
                }
                out.push(row);
            }
            return Ok(out);
        }

        // normSack: divide each __sack by sum of all sacks
        if *norm_sack {
            let sum: f64 = input
                .iter()
                .filter_map(|row| {
                    row.get("__sack").and_then(|v| match v {
                        Value::Float(f) => Some(*f),
                        Value::Int(i) => Some(*i as f64),
                        _ => None,
                    })
                })
                .sum();

            let count = input.len();
            let mut out = Vec::with_capacity(count);
            for mut row in input {
                if let Some(sack_val) = row.get("__sack").cloned() {
                    let sack_f = match &sack_val {
                        Value::Float(f) => *f,
                        Value::Int(i) => *i as f64,
                        _ => 0.0,
                    };
                    let normalized = if sum == 0.0 || sum.is_infinite() {
                        1.0 / count as f64
                    } else {
                        sack_f / sum
                    };
                    row.insert("__sack".to_string(), Value::Float(normalized));
                }
                if let Some(val) = row.get(current_alias.0.as_str()).cloned() {
                    row.insert(out_alias.0.clone(), val);
                }
                out.push(row);
            }
            return Ok(out);
        }

        // Merge mode: group by current_alias value, merge sacks
        if let Some(merge_op) = sack_merge_op {
            let mut groups: IndexMap<String, (Row, f64, i64)> = IndexMap::new();
            for row in &input {
                let key = row
                    .get(current_alias.0.as_str())
                    .map(|v| format!("{v:?}"))
                    .unwrap_or_default();
                let sack_f = row
                    .get("__sack")
                    .and_then(|v| match v {
                        Value::Float(f) => Some(*f),
                        Value::Int(i) => Some(*i as f64),
                        _ => None,
                    })
                    .unwrap_or(0.0);
                let entry = groups.entry(key).or_insert_with(|| (row.clone(), 0.0, 0));
                entry.2 += 1;
                match merge_op.as_str() {
                    "sum" => entry.1 += sack_f,
                    "mult" => {
                        if entry.2 == 1 {
                            entry.1 = sack_f;
                        } else {
                            entry.1 *= sack_f;
                        }
                    }
                    "min" => {
                        if entry.2 == 1 || sack_f < entry.1 {
                            entry.1 = sack_f;
                        }
                    }
                    "max" => {
                        if entry.2 == 1 || sack_f > entry.1 {
                            entry.1 = sack_f;
                        }
                    }
                    "minus" => {
                        if entry.2 == 1 {
                            entry.1 = sack_f;
                        } else {
                            entry.1 -= sack_f;
                        }
                    }
                    "div" => {
                        if entry.2 == 1 {
                            entry.1 = sack_f;
                        } else if sack_f != 0.0 {
                            entry.1 /= sack_f;
                        }
                    }
                    _ => {
                        entry.1 = sack_f;
                    }
                }
            }

            let mut out = Vec::new();
            for (_key, (mut row, merged_sack, bulk)) in groups {
                row.insert("__sack".to_string(), Value::Float(merged_sack));
                if let Some(val) = row.get(current_alias.0.as_str()).cloned() {
                    row.insert(out_alias.0.clone(), val);
                }
                if *with_bulk {
                    for _ in 0..bulk {
                        out.push(row.clone());
                    }
                } else {
                    out.push(row);
                }
            }
            return Ok(out);
        }

        // Fallback: pass-through
        let mut out = Vec::with_capacity(input.len());
        for mut row in input {
            if let Some(val) = row.get(current_alias.0.as_str()).cloned() {
                row.insert(out_alias.0.clone(), val);
            }
            out.push(row);
        }
        Ok(out)
    }
}
