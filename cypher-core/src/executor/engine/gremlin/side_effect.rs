//! Gremlin executor: `side_effect` step implementations.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

#[allow(unused_imports)]
use super::helpers::*;

impl<'a> QueryExecutor<'a> {
    /// Execute `GremlinSideEffect` — run `effect` for each input row (discard
    /// its output) and pass the original rows through unchanged.
    pub(crate) fn exec_gremlin_side_effect(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinSideEffect { effect, child } = op else {
            debug_assert!(false, "expected GremlinSideEffect operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinSideEffect operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        for row in &input {
            cx.budget.check_or_err()?;
            self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
            let _discard = self.exec_operator(cx, effect)?;
            self.foreach_seed.borrow_mut().take();
        }
        Ok(input)
    }

    /// Execute `Cap` — emit accumulated side-effect values for the given keys.
    ///
    /// Executes the child plan (which should contain `GremlinAggregate` operators
    /// that populate `__gremlin_agg_<key>` columns) and then collects those
    /// accumulated values into a single result row.
    pub(crate) fn exec_cap(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Cap {
            keys,
            child,
            reductions,
        } = op
        else {
            debug_assert!(false, "expected Cap operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Cap operator".into(),
            }));
        };

        let child_rows = self.exec_operator(cx, child)?;

        // Collect aggregated values for each key from the child rows.
        // When aggregate runs in a global scope, every row carries the same
        // accumulated list, so taking the first is correct.  When aggregate
        // runs inside local(), each row carries a *partial* single-item list.
        // We detect the latter case and merge partial lists.
        // For group()/groupCount(), the value is a Map — maps are merged by
        // key with last-writer-wins.
        let mut row = Row::new();
        for key in keys {
            let agg_col = format!("__gremlin_agg_{key}");

            // Check the side-effect store first — it has the authoritative
            // accumulated value for aggregate() / local(aggregate()) patterns.
            let se_val = self.gremlin_side_effects.borrow().get(&agg_col).cloned();
            let agg_vals: Vec<&Value> = child_rows.iter().filter_map(|r| r.get(&agg_col)).collect();
            if agg_col == "__gremlin_agg_x" {
                append_gremlin_debug_log(&format!(
                    "[cap:x] child_rows={} agg_vals={} stored={}",
                    child_rows.len(),
                    agg_vals.len(),
                    match se_val.as_ref() {
                        Some(Value::Map(m)) => format!("map:{}", m.len()),
                        Some(Value::List(items)) => format!("list:{}", items.len()),
                        Some(other) => other.type_name().to_string(),
                        None => "none".to_string(),
                    }
                ));
            }
            if agg_vals.is_empty() {
                if let Some(stored) = se_val {
                    row.insert(key.clone(), stored);
                } else {
                    row.insert(key.clone(), Value::List(Vec::new()));
                }
                continue;
            }
            // If there is only one distinct value (all rows have the same),
            // take it directly.  Otherwise merge partial lists/maps.
            let first = agg_vals[0];
            let all_same = agg_vals.iter().all(|v| *v == first);
            let cap_value = if let Some(stored) = &se_val {
                // For List aggregations, prefer the side-effect store which
                // has the authoritative accumulated value (handles local(aggregate)).
                if matches!(stored, Value::List(_) | Value::Map(_)) {
                    stored.clone()
                } else if all_same {
                    first.clone()
                } else if matches!(first, Value::Map(_)) {
                    // Merge maps from rows
                    let mut merged: IndexMap<Box<str>, Value> = IndexMap::new();
                    for v in &agg_vals {
                        if let Value::Map(m) = v {
                            for (mk, mv) in m {
                                match (merged.get(mk), mv) {
                                    (Some(Value::Int(existing)), Value::Int(incoming)) => {
                                        merged.insert(mk.clone(), Value::Int(existing + incoming));
                                    }
                                    (Some(Value::Float(existing)), Value::Float(incoming)) => {
                                        merged
                                            .insert(mk.clone(), Value::Float(existing + incoming));
                                    }
                                    _ => {
                                        merged.insert(mk.clone(), mv.clone());
                                    }
                                }
                            }
                        }
                    }
                    Value::Map(merged)
                } else {
                    stored.clone()
                }
            } else if all_same {
                first.clone()
            } else {
                // Merge: for Lists, concatenate; for Maps, union by key.
                match first {
                    Value::List(_) => {
                        let mut merged = Vec::new();
                        for v in &agg_vals {
                            if let Value::List(items) = v {
                                merged.extend(items.iter().cloned());
                            }
                        }
                        Value::List(merged)
                    }
                    Value::Map(_) => {
                        let mut merged: IndexMap<Box<str>, Value> = IndexMap::new();
                        for v in &agg_vals {
                            if let Value::Map(m) = v {
                                for (mk, mv) in m {
                                    // For integer values (groupCount), add counts
                                    // instead of overwriting so accumulation across
                                    // traverser branches works correctly.
                                    match (merged.get(mk), mv) {
                                        (Some(Value::Int(existing)), Value::Int(incoming)) => {
                                            merged.insert(
                                                mk.clone(),
                                                Value::Int(existing + incoming),
                                            );
                                        }
                                        (Some(Value::Float(existing)), Value::Float(incoming)) => {
                                            merged.insert(
                                                mk.clone(),
                                                Value::Float(existing + incoming),
                                            );
                                        }
                                        _ => {
                                            merged.insert(mk.clone(), mv.clone());
                                        }
                                    }
                                }
                            }
                        }
                        Value::Map(merged)
                    }
                    _ => first.clone(),
                }
            };

            // Check for a reduction operator (from withSideEffect with Operator.*)
            let reduction = reductions.iter().find(|(k, _, _)| k == key);
            if let Some((_k, op, init)) = reduction {
                if op == "addAll" || op == "assign" {
                    // addAll / assign: the cap value is already the full list
                    // (initial values + aggregated values). Return as-is.
                    row.insert(key.clone(), cap_value);
                } else if let Value::List(items) = &cap_value {
                    // Handle boolean reductions (and/or)
                    if op == "and" || op == "or" {
                        let init_bool = *init != 0.0;
                        let mut acc = init_bool;
                        for item in items {
                            let v = match item {
                                Value::Bool(b) => *b,
                                Value::Int(n) => *n != 0,
                                _ => continue,
                            };
                            match op.as_str() {
                                "and" => acc = acc && v,
                                "or" => acc = acc || v,
                                _ => {}
                            }
                        }
                        row.insert(key.clone(), Value::Bool(acc));
                    } else {
                        // Numeric reductions
                        let mut acc = *init;
                        let init_is_int = (*init - init.round()).abs() < 1e-12;
                        let mut all_int = init_is_int;
                        for item in items {
                            let v = match item {
                                Value::Int(n) => *n as f64,
                                Value::Float(f) => {
                                    all_int = false;
                                    *f
                                }
                                _ => continue,
                            };
                            match op.as_str() {
                                "sum" | "add" => acc += v,
                                "minus" | "sub" => acc -= v,
                                "mult" | "multiply" => acc *= v,
                                "div" | "divide" => {
                                    if v != 0.0 {
                                        all_int = false;
                                        acc /= v;
                                    }
                                }
                                "min" => {
                                    if v < acc {
                                        acc = v;
                                    }
                                }
                                "max" => {
                                    if v > acc {
                                        acc = v;
                                    }
                                }
                                _ => acc += v,
                            }
                        }
                        if all_int && !matches!(op.as_str(), "div" | "divide") {
                            row.insert(key.clone(), Value::Int(acc as i64));
                        } else {
                            row.insert(key.clone(), Value::Float(acc));
                        }
                    }
                } else {
                    row.insert(key.clone(), cap_value);
                }
            } else {
                row.insert(key.clone(), cap_value);
            }
        }
        // For multi-key cap, also create a combined Map value so that
        // downstream unfold() can iterate over the {key: value} entries.
        if keys.len() > 1 {
            let mut map: IndexMap<Box<str>, Value> = IndexMap::new();
            for key in keys {
                if let Some(v) = row.get(key.as_str()) {
                    map.insert(key.clone().into(), v.clone());
                }
            }
            let map_alias = keys.join("_");
            row.insert(map_alias, Value::Map(map));
        }
        Ok(vec![row])
    }

    /// Execute `GremlinAggregate` — collect traverser values into a
    /// `__gremlin_agg_<key>` column for later retrieval by `Cap`.
    pub(crate) fn exec_gremlin_aggregate(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinAggregate {
            key,
            child,
            value_alias,
            by_keys,
            by_traversal,
            productive,
            set_semantics,
            assign_semantics,
        } = op
        else {
            debug_assert!(false, "expected GremlinAggregate operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinAggregate operator".into(),
            }));
        };

        let mut input = self.exec_operator(cx, child)?;
        let agg_col = format!("__gremlin_agg_{key}");

        // Pre-seed from withSideEffect initial value on the very first call.
        // The initial value is stored in __side_effect_{key} column by the
        // compiler. Only pre-seed if the side-effect store has no entry yet.
        let mut collected: Vec<Value> = Vec::new();
        if !*assign_semantics {
            // Pre-seed from withSideEffect initial value on the very first call.
            // Skipped for assign_semantics since assign REPLACES (not appends).
            let se = self.gremlin_side_effects.borrow();
            if se.contains_key(&agg_col) {
                // Already seeded from a prior call — don't re-seed.
            } else {
                let se_col = format!("__side_effect_{key}");
                if let Some(row) = input.first() {
                    if let Some(Value::List(items)) = row.get(&se_col) {
                        collected.extend(items.iter().cloned());
                    }
                }
            }
        }
        for row in &input {
            if let Some((trav_op, trav_alias)) = by_traversal {
                // by(traversal) modulation: run the sub-traversal per traverser
                self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                let trav_rows = self.exec_operator(cx, trav_op)?;
                self.foreach_seed.borrow_mut().take();
                if let Some(trav_row) = trav_rows.first() {
                    if let Some(val) = trav_row.get(trav_alias.as_str()) {
                        if *productive || !matches!(val, Value::Null) {
                            collected.push(val.clone());
                        }
                    } else if *productive {
                        collected.push(Value::Null);
                    }
                } else if *productive {
                    collected.push(Value::Null);
                }
            } else if by_keys.len() >= 2 {
                // Multiple by() keys — collect a map of {key1: val1, key2: val2, ...}
                let mut map = IndexMap::new();
                for by_prop in by_keys {
                    let val = self.eval_gremlin_by_key(cx, row, value_alias, by_prop)?;
                    map.insert(by_prop.clone().into_boxed_str(), val);
                }
                collected.push(Value::Map(map));
            } else if let Some(by_prop) = by_keys.first() {
                // Single by("prop") modulation: resolve property from the node/edge value
                let projected = self.eval_gremlin_by_key(cx, row, value_alias, by_prop)?;
                // Skip null values unless ProductiveByStrategy is active
                if *productive || !matches!(projected, Value::Null) {
                    collected.push(projected);
                }
            } else if let Some(val) = row.get(value_alias.as_str()) {
                collected.push(val.clone());
            }
        }

        // Stamp the accumulated list onto every output row.
        // If a row already carries a value for this agg column (e.g. from a
        // previous local(aggregate("a")) step), extend it rather than overwrite.
        //
        // Also accumulate into the side-effect store so that cap() can
        // retrieve the full list even when downstream steps filter out rows.
        {
            let mut se = self.gremlin_side_effects.borrow_mut();
            if *assign_semantics {
                // Operator.assign: REPLACE the accumulated list with the current
                // traverser's values. The last aggregate call wins.
                se.insert(agg_col.clone(), Value::List(collected.clone()));
            } else if let Some(Value::List(existing)) = se.get_mut(&agg_col) {
                if *set_semantics {
                    for v in &collected {
                        if !existing.contains(v) {
                            existing.push(v.clone());
                        }
                    }
                } else {
                    existing.extend(collected.iter().cloned());
                }
            } else {
                let stored = if *set_semantics {
                    let mut deduped = Vec::new();
                    for v in &collected {
                        if !deduped.contains(v) {
                            deduped.push(v.clone());
                        }
                    }
                    deduped
                } else {
                    collected.clone()
                };
                se.insert(agg_col.clone(), Value::List(stored));
            }
        }
        if input.is_empty() {
            // No traversers passed the filter (all edges already visited, etc.).
            // The side-effect store was already updated above, so cap() can
            // retrieve the accumulated value from gremlin_side_effects.
            // Return an empty stream so that downstream steps (like otherV())
            // inside a repeat body do not see a sentinel row that is missing
            // intermediate step variables (e.g. the edge alias from bothE).
            return Ok(vec![]);
        }
        for row in &mut input {
            if *assign_semantics {
                // Operator.assign: REPLACE — row carries only the current values.
                row.insert(agg_col.clone(), Value::List(collected.clone()));
            } else if let Some(Value::List(existing)) = row.get(&agg_col).cloned() {
                let mut merged = existing;
                if *set_semantics {
                    for v in &collected {
                        if !merged.contains(v) {
                            merged.push(v.clone());
                        }
                    }
                } else {
                    merged.extend(collected.iter().cloned());
                }
                row.insert(agg_col.clone(), Value::List(merged));
            } else {
                let stored = if *set_semantics {
                    let mut deduped = Vec::new();
                    for v in &collected {
                        if !deduped.contains(v) {
                            deduped.push(v.clone());
                        }
                    }
                    deduped
                } else {
                    collected.clone()
                };
                row.insert(agg_col.clone(), Value::List(stored));
            }
        }

        Ok(input)
    }
}
