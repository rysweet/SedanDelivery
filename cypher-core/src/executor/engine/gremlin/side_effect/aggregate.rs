//! `aggregate()` / `store()` — collect traverser values into the
//! side-effect store under a `__gremlin_agg_<key>` column for later
//! retrieval by `cap()`.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

#[allow(unused_imports)]
use super::super::helpers::*;

impl<'a> QueryExecutor<'a> {
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
