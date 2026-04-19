//! `cap()` — emit accumulated side-effect values for the given keys.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

#[allow(unused_imports)]
use super::super::helpers::*;

impl<'a> QueryExecutor<'a> {
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
}
