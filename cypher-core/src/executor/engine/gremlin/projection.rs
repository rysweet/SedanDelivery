//! Gremlin executor: `projection` step implementations.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

#[allow(unused_imports)]
use super::helpers::*;

impl<'a> QueryExecutor<'a> {
    /// Execute `GremlinSample` — barrier sampling of the traverser stream using
    /// Gremlin's weighted roulette semantics.
    pub(crate) fn exec_gremlin_sample(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinSample {
            count,
            weight,
            seed,
            child,
        } = op
        else {
            debug_assert!(false, "expected GremlinSample operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinSample operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut weighted_rows = Vec::with_capacity(input.len());
        for row in input {
            cx.budget.check_or_err()?;
            let row_weight = if let Some(weight_expr) = weight {
                let filter_ctx = self.build_filter_ctx(cx.graph, &row)?;
                match self.eval_with_storage(cx.graph, weight_expr, &row, &filter_ctx)? {
                    Value::Int(n) => n as f64,
                    Value::Float(f) => f,
                    Value::Null => continue,
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "sample().by() numeric weight".into(),
                            actual: other.type_name().to_string(),
                        }))
                    }
                }
            } else {
                1.0
            };
            if row_weight.is_sign_negative() || !row_weight.is_finite() {
                continue;
            }
            weighted_rows.push((row, row_weight));
        }

        if weighted_rows.len() <= *count {
            return Ok(weighted_rows.into_iter().map(|(row, _)| row).collect());
        }

        let mut total_weight: f64 = weighted_rows.iter().map(|(_, weight)| *weight).sum();
        let seed = seed.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as i64
        });
        let mut rng = JavaCompatibleRandom::new(seed);
        let mut sampled = Vec::with_capacity(*count);

        while sampled.len() < *count && !weighted_rows.is_empty() {
            let mut running_total_weight = total_weight;
            let mut selected = None;
            for (idx, (_, row_weight)) in weighted_rows.iter().enumerate() {
                cx.budget.check_or_err()?;
                let draw = rng.next_double();
                if draw <= (*row_weight / running_total_weight.max(f64::MIN_POSITIVE)) {
                    selected = Some(idx);
                    break;
                }
                running_total_weight -= *row_weight;
            }
            if let Some(idx) = selected {
                let (row, row_weight) = weighted_rows.remove(idx);
                total_weight -= row_weight;
                sampled.push(row);
            } else {
                break;
            }
        }

        Ok(sampled)
    }

    /// Execute `GremlinMapSlice` — slice Map/List values in each row.
    pub(crate) fn exec_gremlin_map_slice(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        use indexmap::IndexMap;

        let Operator::GremlinMapSlice {
            alias,
            lo,
            hi,
            child,
        } = op
        else {
            debug_assert!(false, "expected GremlinMapSlice operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinMapSlice operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());
        for mut row in input {
            cx.budget.check_or_err()?;
            if let Some(val) = row.get(&alias.0) {
                let sliced = match val {
                    Value::Map(m) => {
                        let new_map: IndexMap<Box<str>, Value> = m
                            .iter()
                            .skip(*lo)
                            .take(hi - lo)
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        Value::Map(new_map)
                    }
                    Value::List(l) => {
                        let new_list: Vec<Value> =
                            l.iter().skip(*lo).take(hi - lo).cloned().collect();
                        Value::List(new_list)
                    }
                    other => other.clone(),
                };
                row.insert(alias.0.clone(), sliced);
            }
            out.push(row);
        }
        Ok(out)
    }

    /// Execute `GremlinIndex` — pair each element with its positional index.
    pub(crate) fn exec_gremlin_index(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinIndex {
            child,
            alias,
            out_alias,
            with_map,
        } = op
        else {
            debug_assert!(false, "expected GremlinIndex operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinIndex operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        // Check if input is a single row containing a List — index its elements
        if input.len() == 1 {
            if let Some(Value::List(items)) = input[0].get(alias.as_str()) {
                if *with_map {
                    // Produce a single Map {0: elem0, 1: elem1, ...}
                    let mut map = IndexMap::new();
                    for (i, item) in items.iter().enumerate() {
                        map.insert(format!("{i}").into_boxed_str(), item.clone());
                    }
                    let mut row = Row::new();
                    row.insert(out_alias.0.clone(), Value::Map(map));
                    out.push(row);
                } else {
                    // Produce a single row containing the list of [element, index] pairs.
                    // TinkerPop index() on a folded list returns a list of pairs so that
                    // subsequent unfold() yields each pair as a List value rather than
                    // unfolding the pairs themselves.
                    let pairs: Vec<Value> = items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| Value::List(vec![item.clone(), Value::Int(i as i64)]))
                        .collect();
                    let mut row = input[0].clone();
                    row.insert(out_alias.0.clone(), Value::List(pairs));
                    out.push(row);
                }
                return Ok(out);
            }
        }

        // Stream mode: each individual traverser gets index 0 (per TinkerPop semantics).
        if *with_map {
            // with(map) mode: each individual traverser → {0: element}
            for row in input.iter() {
                if let Some(val) = row.get(alias.as_str()) {
                    let mut map = IndexMap::new();
                    map.insert("0".to_string().into_boxed_str(), val.clone());
                    let mut new_row = Row::new();
                    new_row.insert(out_alias.0.clone(), Value::Map(map));
                    out.push(new_row);
                }
            }
        } else {
            // Collect all [element, 0] pairs into a single list for unfold().
            let mut pairs = Vec::with_capacity(input.len());
            for row in input.iter() {
                cx.budget.check_or_err()?;
                let val = row.get(alias.as_str()).cloned().unwrap_or(Value::Null);
                pairs.push(Value::List(vec![val, Value::Int(0)]));
            }
            let mut row = Row::new();
            row.insert(out_alias.0.clone(), Value::List(pairs));
            out.push(row);
        }
        Ok(out)
    }

    /// Execute `GremlinProject` — evaluate sub-traversals per key for each input row.
    pub(crate) fn exec_gremlin_project(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinProject {
            entries,
            child,
            current_alias,
            out_alias,
            productive,
        } = op
        else {
            debug_assert!(false, "expected GremlinProject operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinProject operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());
        for row in &input {
            cx.budget.check_or_err()?;
            let mut map = IndexMap::new();
            for (key, sub_op, out_alias, by_is_identity) in entries {
                // Feed the current row as a seed and evaluate the sub-traversal
                self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                let sub_rows = self.exec_operator(cx, sub_op)?;
                self.foreach_seed.borrow_mut().take();
                // Take the first result value (project picks the first traverser)
                let val = if let Some(sub_row) = sub_rows.first() {
                    sub_row
                        .get(out_alias.as_str())
                        .or_else(|| {
                            // Fall back to current_alias if out_alias not found
                            sub_row.get(current_alias.as_str())
                        })
                        .cloned()
                        .unwrap_or(Value::Null)
                } else {
                    Value::Null
                };
                // Omit keys where by-modulation produced Null (property
                // doesn't exist), unless ProductiveByStrategy is active or
                // the by() is the identity (empty by()). When by_is_identity
                // is true, a null value means the traverser itself is null —
                // this is a legitimate null that must appear in the result map.
                if matches!(val, Value::Null) && !*productive && !by_is_identity {
                    continue;
                }
                map.insert(key.clone().into_boxed_str(), val);
            }
            // Store the projected Map as both individual columns and a combined Map
            let map_val = Value::Map(map.clone());
            let mut out_row = row.clone();
            for (k, v) in &map {
                out_row.insert(k.to_string(), v.clone());
            }
            out_row.insert(out_alias.0.clone(), map_val);
            out.push(out_row);
        }
        Ok(out)
    }

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
