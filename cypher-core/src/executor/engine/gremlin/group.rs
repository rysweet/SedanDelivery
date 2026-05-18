//! Gremlin executor: `group` step implementations.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

#[allow(unused_imports)]
use super::helpers::*;

impl<'a> QueryExecutor<'a> {
    /// Execute `GremlinGroup` — group all input traversers into a single Map.
    ///
    /// For each input row, evaluates `key_expr` and `value_expr`. Groups
    /// values by key and applies the aggregation function. Returns a single
    /// row containing one Map value.
    pub(crate) fn exec_gremlin_group(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        use crate::planner::AggFunction;
        use indexmap::IndexMap;

        let Operator::GremlinGroup {
            key_expr,
            value_expr,
            agg_fn,
            alias,
            side_effect_key,
            key_traversal,
            value_traversal,
            productive,
            key_is_identity,
            explicit_fold,
            child,
        } = op
        else {
            debug_assert!(false, "expected GremlinGroup operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinGroup operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;

        // Collect (key_str, value) pairs from input rows.
        // Use Box<str> keys since Value doesn't implement Hash.
        let mut groups: IndexMap<Box<str>, Vec<Value>> = IndexMap::new();
        // Also collect raw rows per group for value_traversal (which runs on group members)
        let mut group_rows: IndexMap<Box<str>, Vec<Row>> = IndexMap::new();

        // Extract withSideEffect initial map from input rows so we can
        // merge its entries into the final result_map after traversal completes.
        // When group("a") follows withSideEffect("a", initialMap), the compiler
        // injects the initial map into rows as __side_effect_a via a Project operator.
        let se_initial_map: Option<IndexMap<Box<str>, Value>> = if let Some(key) = side_effect_key {
            let se_col = format!("__side_effect_{key}");
            let result = input.first().and_then(|row| {
                if let Some(Value::Map(m)) = row.get(&se_col) {
                    Some(m.clone())
                } else {
                    None
                }
            });
            result
        } else {
            None
        };

        // Check if value_expr differs from key_expr (i.e., a value-by modulation is present).
        let has_value_by = value_expr != key_expr || value_traversal.is_some();

        for row in &input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, row)?;

            // Compute group key: use traversal if available, else eval expression.
            let key_val = if let Some((key_op, key_alias)) = key_traversal {
                self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                let key_rows = self.exec_operator(cx, key_op)?;
                self.foreach_seed.borrow_mut().take();
                key_rows
                    .first()
                    .and_then(|r| r.get(key_alias.0.as_str()).cloned())
                    .or_else(|| key_rows.first().and_then(|r| r.values().next().cloned()))
                    .unwrap_or(Value::Null)
            } else {
                self.eval_with_storage(cx.graph, key_expr, row, &ctx)
                    .unwrap_or(Value::Null)
            };
            // Skip rows where the group key evaluates to Null (unproductive by),
            // unless ProductiveByStrategy is active or the key is the identity
            // (traverser itself). When key_is_identity is true, the traverser IS
            // the key — a null traverser is a valid group key and must be counted.
            if matches!(key_val, Value::Null) && !*productive && !*key_is_identity {
                continue;
            }

            let key_str = value_to_map_key(&key_val);

            if value_traversal.is_some() {
                // For value_traversal mode, collect rows; we'll run traversal per group later
                group_rows.entry(key_str).or_default().push(row.clone());
            } else {
                // For expression-based value, evaluate per row
                let val = self
                    .eval_with_storage(cx.graph, value_expr, row, &ctx)
                    .unwrap_or(Value::Null);
                let entry = groups.entry(key_str).or_default();
                if has_value_by && matches!(val, Value::Null) {
                    // Key still gets created with empty vec
                } else {
                    entry.push(val);
                }
            }
        }

        // For value_traversal, run the traversal per group with all members as seed
        if let Some((val_op, val_alias)) = value_traversal {
            for (key_str, rows) in &group_rows {
                self.foreach_seed.borrow_mut().replace(rows.clone());
                let val_rows = self.exec_operator(cx, val_op)?;
                self.foreach_seed.borrow_mut().take();
                let mut values = Vec::new();
                for r in &val_rows {
                    if let Some(v) = r.get(val_alias.0.as_str()).cloned() {
                        values.push(v);
                    } else if let Some(v) = r.values().next().cloned() {
                        values.push(v);
                    }
                }
                groups.insert(key_str.clone(), values);
            }
        }

        // Merge withSideEffect initial values into groups.
        // For keys already in groups (traversal matched): prepend initial values.
        // For keys only in initial map (no matching vertex): use initial values as-is.
        if let Some(init_map) = se_initial_map {
            for (k, v) in init_map {
                let init_vals: Vec<Value> = match v {
                    Value::List(items) => items,
                    other => vec![other],
                };
                let entry = groups.entry(k).or_default();
                // Prepend so initial values come before traversal values.
                let mut merged = init_vals;
                merged.append(entry);
                *entry = merged;
            }
        }

        // Apply aggregation function to each group.
        let has_value_trav = value_traversal.is_some();
        let mut result_map: IndexMap<Box<str>, Value> = IndexMap::new();
        for (key_str, values) in groups {
            // When value traversal produces no results for a group member,
            // skip the entry for reducing aggregations and non-explicit Collect.
            if has_value_trav
                && values.is_empty()
                && (matches!(
                    agg_fn,
                    AggFunction::Sum | AggFunction::Min | AggFunction::Max | AggFunction::Avg
                ) || (matches!(agg_fn, AggFunction::Collect) && !*explicit_fold))
            {
                continue;
            }
            let agg_result = if has_value_trav {
                // When value_traversal is set, the traversal may produce a single
                // aggregated result (e.g., constant(1), count(), sum()) or multiple
                // per-member results to be collected. Apply agg_fn to the values,
                // then unwrap single scalar results for the single-result case.
                match agg_fn {
                    AggFunction::Count => Value::Int(values.len() as i64),
                    AggFunction::Sum => {
                        let mut total = 0i64;
                        let mut float_total = 0.0f64;
                        let mut use_float = false;
                        for v in &values {
                            match v {
                                Value::Int(i) => {
                                    total = total.checked_add(*i).ok_or_else(|| {
                                        GqlError::Execution(ExecutionError::IntegerOverflow {
                                            context: "Gremlin group sum (value traversal)".into(),
                                        })
                                    })?;
                                }
                                Value::Float(f) => {
                                    use_float = true;
                                    float_total += f;
                                }
                                _ => {}
                            }
                        }
                        if use_float {
                            Value::Float(float_total + total as f64)
                        } else {
                            Value::Int(total)
                        }
                    }
                    AggFunction::Collect => {
                        if *explicit_fold {
                            // Explicit fold(): always produce a List
                            Value::List(values)
                        } else if values.len() == 1 {
                            // Single result from value traversal: unwrap unconditionally.
                            // This handles both scalar results (e.g., constant(1)) and
                            // list results (e.g., select(Column.values) returning a List).
                            // The value traversal already produced the final value.
                            values.into_iter().next().unwrap_or(Value::Null)
                        } else {
                            Value::List(values)
                        }
                    }
                    _ => Value::List(values),
                }
            } else {
                match agg_fn {
                    AggFunction::Count => Value::Int(values.len() as i64),
                    AggFunction::Collect => Value::List(values),
                    AggFunction::Sum => {
                        let mut total = 0i64;
                        let mut float_total = 0.0f64;
                        let mut use_float = false;
                        for v in &values {
                            match v {
                                Value::Int(i) => {
                                    total = total.checked_add(*i).ok_or_else(|| {
                                        GqlError::Execution(ExecutionError::IntegerOverflow {
                                            context: "Gremlin group sum".into(),
                                        })
                                    })?;
                                }
                                Value::Float(f) => {
                                    use_float = true;
                                    float_total += f;
                                }
                                _ => {}
                            }
                        }
                        if use_float {
                            Value::Float(float_total + total as f64)
                        } else {
                            Value::Int(total)
                        }
                    }
                    _ => Value::List(values),
                }
            };
            result_map.insert(key_str, agg_result);
        }

        // Named side-effect: store the Map on every input row and pass
        // traversers through instead of collapsing into a single Map row.
        if let Some(key) = side_effect_key {
            let agg_col = format!("__gremlin_agg_{key}");
            let mut output = input;
            if output.is_empty() {
                // Even with no input, the side-effect must remain accessible via
                // cap(). Preserve any previously accumulated named side-effect map
                // instead of overwriting it with an empty map on an exhausted
                // repeat() iteration.
                let map_val = if let Some(Value::Map(existing)) =
                    self.gremlin_side_effects.borrow().get(&agg_col).cloned()
                {
                    let mut merged = existing;
                    for (mk, mv) in result_map {
                        merged.insert(mk, mv);
                    }
                    Value::Map(merged)
                } else {
                    Value::Map(result_map)
                };
                let mut sentinel = Row::new();
                sentinel.insert(agg_col.clone(), map_val.clone());
                // Store in side-effect store for cap() retrieval
                self.gremlin_side_effects
                    .borrow_mut()
                    .insert(agg_col, map_val);
                if key == "x" {
                    append_gremlin_debug_log(&format!(
                        "[group:x] empty-input sentinel map-size={}",
                        match sentinel.get("__gremlin_agg_x") {
                            Some(Value::Map(m)) => m.len(),
                            _ => 0,
                        }
                    ));
                }
                return Ok(vec![sentinel]);
            }
            // Merge new counts with any existing aggregation carried from
            // a previous repeat iteration so counts accumulate.
            let map_val = {
                let mut merged = result_map;
                let has_row_existing = output.first().and_then(|f| f.get(&agg_col)).is_some();
                if has_row_existing {
                    // Row carries forward the accumulated map (repeat case).
                    if let Some(Value::Map(existing)) = output.first().and_then(|f| f.get(&agg_col))
                    {
                        for (ek, ev) in existing {
                            merge_gremlin_map_entry(&mut merged, ek, ev)?;
                        }
                    }
                } else if let Some(Value::Map(existing)) =
                    self.gremlin_side_effects.borrow().get(&agg_col).cloned()
                {
                    for (ek, ev) in existing {
                        merge_gremlin_map_entry(&mut merged, &ek, &ev)?;
                    }
                }
                Value::Map(merged)
            };
            // Store in side-effect store for cap() retrieval
            self.gremlin_side_effects
                .borrow_mut()
                .insert(agg_col.clone(), map_val.clone());
            if key == "x" {
                let sample_keys = match &map_val {
                    Value::Map(m) => m
                        .keys()
                        .take(6)
                        .map(|k| k.to_string())
                        .collect::<Vec<_>>()
                        .join(" | "),
                    _ => String::new(),
                };
                let sample_value = match &map_val {
                    Value::Map(m) => m
                        .values()
                        .next()
                        .map(|value| summarize_gremlin_value_shape(value, 3))
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                append_gremlin_debug_log(&format!(
                    "[group:x] output_rows={} map-size={} sample={} value={}",
                    output.len(),
                    match &map_val {
                        Value::Map(m) => m.len(),
                        _ => 0,
                    },
                    sample_keys,
                    sample_value
                ));
            }
            for row in &mut output {
                row.insert(agg_col.clone(), map_val.clone());
            }
            return Ok(output);
        }

        let mut out_row = Row::new();
        out_row.insert(alias.0.clone(), Value::Map(result_map));
        cx.budget.record_row();
        Ok(vec![out_row])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, Identifier, Label};
    use crate::executor::budget::ExecutionBudget;
    use crate::executor::engine::QueryExecutor;
    use crate::planner::{AggFunction, Operator};
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use indexmap::IndexMap;

    fn make_store() -> InMemoryGraphStore {
        let s = InMemoryGraphStore::new();
        s.create_graph("g").unwrap();
        s
    }

    /// Build a GremlinGroup operator that groups by a constant key "all"
    /// and sums the given property from child rows.
    fn make_sum_group(prop: &str, child: Operator) -> Operator {
        let alias = Identifier::new("__group");
        let src_alias = Identifier::new("v_1");
        // key_expr: constant string "all" so everything goes to one group
        let key_expr = Expr::Str("all".into());
        // value_expr: property access on src_alias.prop
        let value_expr = Expr::Property(
            Box::new(Expr::Variable(src_alias)),
            smol_str::SmolStr::new(prop),
        );
        Operator::GremlinGroup {
            key_expr,
            value_expr,
            agg_fn: AggFunction::Sum,
            alias,
            side_effect_key: None,
            key_traversal: None,
            value_traversal: None,
            productive: false,
            key_is_identity: false,
            explicit_fold: false,
            child: Box::new(child),
        }
    }

    // ── Bug #1205: group sum must use checked_add for i64 overflow ─────
    //
    // Currently the sum aggregation in exec_gremlin_group uses unchecked
    // `total += i` which wraps on i64 overflow. After fix: use checked_add
    // and return ExecutionError::IntegerOverflow on overflow.

    #[test]
    fn group_sum_normal_values() {
        // Regression: normal sum should still work.
        let s = make_store();
        let mut props1 = IndexMap::new();
        props1.insert("val".to_string(), Value::Int(10));
        s.create_node("g", vec!["T".to_string()], props1).unwrap();
        let mut props2 = IndexMap::new();
        props2.insert("val".to_string(), Value::Int(20));
        s.create_node("g", vec!["T".to_string()], props2).unwrap();

        let child = Operator::NodeScan {
            label: Some(Label::new("T")),
            alias: Identifier::new("v_1"),
        };
        let op = make_sum_group("val", child);

        let exe = QueryExecutor::new(&s);
        let mut budget = ExecutionBudget::unlimited();
        let result = exe.execute("g", &op, &mut budget);
        assert!(result.is_ok(), "normal sum should succeed: {result:?}");
        let rows = result.unwrap().rows;
        assert_eq!(rows.len(), 1);
        // The result map should have key "all" with value Int(30)
        if let Some(Value::Map(m)) = rows[0].get("__group") {
            assert_eq!(m.get("all"), Some(&Value::Int(30)));
        } else {
            panic!("expected Map result, got: {:?}", rows[0]);
        }
    }

    #[test]
    fn group_sum_i64_overflow_returns_error() {
        // Two values near i64::MAX that overflow when added.
        let s = make_store();
        let mut props1 = IndexMap::new();
        props1.insert("val".to_string(), Value::Int(i64::MAX));
        s.create_node("g", vec!["T".to_string()], props1).unwrap();
        let mut props2 = IndexMap::new();
        props2.insert("val".to_string(), Value::Int(1));
        s.create_node("g", vec!["T".to_string()], props2).unwrap();

        let child = Operator::NodeScan {
            label: Some(Label::new("T")),
            alias: Identifier::new("v_1"),
        };
        let op = make_sum_group("val", child);

        let exe = QueryExecutor::new(&s);
        let mut budget = ExecutionBudget::unlimited();
        let result = exe.execute("g", &op, &mut budget);
        assert!(
            result.is_err(),
            "i64 overflow in group sum should return error, got: {result:?}"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(
                err,
                GqlError::Execution(ExecutionError::IntegerOverflow { .. })
            ),
            "expected IntegerOverflow, got: {err:?}"
        );
    }

    #[test]
    fn group_sum_i64_negative_overflow_returns_error() {
        // Negative overflow: i64::MIN + (-1) overflows.
        let s = make_store();
        let mut props1 = IndexMap::new();
        props1.insert("val".to_string(), Value::Int(i64::MIN));
        s.create_node("g", vec!["T".to_string()], props1).unwrap();
        let mut props2 = IndexMap::new();
        props2.insert("val".to_string(), Value::Int(-1));
        s.create_node("g", vec!["T".to_string()], props2).unwrap();

        let child = Operator::NodeScan {
            label: Some(Label::new("T")),
            alias: Identifier::new("v_1"),
        };
        let op = make_sum_group("val", child);

        let exe = QueryExecutor::new(&s);
        let mut budget = ExecutionBudget::unlimited();
        let result = exe.execute("g", &op, &mut budget);
        assert!(
            result.is_err(),
            "i64 negative overflow in group sum should return error, got: {result:?}"
        );
    }

    #[test]
    fn group_sum_max_value_no_overflow() {
        // i64::MAX + 0 should NOT overflow.
        let s = make_store();
        let mut props1 = IndexMap::new();
        props1.insert("val".to_string(), Value::Int(i64::MAX));
        s.create_node("g", vec!["T".to_string()], props1).unwrap();
        let mut props2 = IndexMap::new();
        props2.insert("val".to_string(), Value::Int(0));
        s.create_node("g", vec!["T".to_string()], props2).unwrap();

        let child = Operator::NodeScan {
            label: Some(Label::new("T")),
            alias: Identifier::new("v_1"),
        };
        let op = make_sum_group("val", child);

        let exe = QueryExecutor::new(&s);
        let mut budget = ExecutionBudget::unlimited();
        let result = exe.execute("g", &op, &mut budget);
        assert!(
            result.is_ok(),
            "i64::MAX + 0 should not overflow: {result:?}"
        );
    }
}
