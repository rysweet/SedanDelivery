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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::budget::ExecutionBudget;
    use crate::executor::engine::{ExecContext, MutationStats, QueryExecutor};
    use crate::storage::{GraphStore, InMemoryGraphStore};

    fn make_store() -> InMemoryGraphStore {
        let s = InMemoryGraphStore::new();
        s.create_graph("g").unwrap();
        s
    }

    fn make_cx<'a, 'b>(
        budget: &'a mut ExecutionBudget,
        stats: &'a mut MutationStats,
    ) -> ExecContext<'a>
    where
        'b: 'a,
    {
        ExecContext {
            graph: "g",
            budget,
            outer_rows: &[],
            stats,
        }
    }

    fn run_cap(
        exe: &QueryExecutor<'_>,
        keys: &[&str],
        reductions: Vec<(String, String, f64)>,
    ) -> Vec<Row> {
        // Use SingleRow as child and pre-populate the side-effect store
        // (covers the "agg_vals empty + stored Some" branch).
        let op = Operator::Cap {
            keys: keys.iter().map(|k| k.to_string()).collect(),
            child: Box::new(Operator::SingleRow),
            reductions,
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let mut cx = make_cx(&mut budget, &mut stats);
        exe.exec_cap(&mut cx, &op).unwrap()
    }

    /// Build a Project that injects `__gremlin_agg_<key>` columns from list
    /// literals so the cap reduction loop sees actual values.
    fn project_agg(items: Vec<(String, Vec<Value>)>) -> Operator {
        use crate::ast::{Expr, Identifier};
        let proj_items: Vec<(Expr, Option<Identifier>)> = items
            .into_iter()
            .map(|(k, vals)| {
                let exprs: Vec<Expr> = vals
                    .into_iter()
                    .map(|v| match v {
                        Value::Int(i) => Expr::Integer(i),
                        Value::Float(f) => Expr::Float(f),
                        Value::Bool(b) => Expr::Bool(b),
                        Value::Str(s) => Expr::Str(s),
                        _ => Expr::Null,
                    })
                    .collect();
                (
                    Expr::List(exprs),
                    Some(Identifier::new(format!("__gremlin_agg_{k}"))),
                )
            })
            .collect();
        Operator::Project {
            items: proj_items,
            child: Box::new(Operator::SingleRow),
        }
    }

    fn run_cap_with_project(
        exe: &QueryExecutor<'_>,
        keys: &[&str],
        reductions: Vec<(String, String, f64)>,
        agg_lists: Vec<(String, Vec<Value>)>,
    ) -> Vec<Row> {
        let op = Operator::Cap {
            keys: keys.iter().map(|k| k.to_string()).collect(),
            child: Box::new(project_agg(agg_lists)),
            reductions,
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let mut cx = make_cx(&mut budget, &mut stats);
        exe.exec_cap(&mut cx, &op).unwrap()
    }

    #[test]
    fn cap_no_aggregate_yields_empty_list() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let rows = run_cap(&exe, &["x"], vec![]);
        assert_eq!(rows.len(), 1);
        let v = rows[0].get("x").unwrap();
        match v {
            Value::List(items) => assert!(items.is_empty()),
            other => panic!("expected empty List, got {other:?}"),
        }
    }

    #[test]
    fn cap_with_stored_list_returns_it() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        exe.gremlin_side_effects.borrow_mut().insert(
            "__gremlin_agg_x".into(),
            Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        );
        let rows = run_cap(&exe, &["x"], vec![]);
        match rows[0].get("x").unwrap() {
            Value::List(items) => assert_eq!(items.len(), 3),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn cap_sum_reduction_int() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let rows = run_cap_with_project(
            &exe,
            &["s"],
            vec![("s".into(), "sum".into(), 0.0)],
            vec![(
                "s".into(),
                vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)],
            )],
        );
        assert_eq!(rows[0].get("s").unwrap(), &Value::Int(10));
    }

    #[test]
    fn cap_sum_reduction_with_float_promotes() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let rows = run_cap_with_project(
            &exe,
            &["s"],
            vec![("s".into(), "sum".into(), 0.0)],
            vec![("s".into(), vec![Value::Int(1), Value::Float(0.5)])],
        );
        match rows[0].get("s").unwrap() {
            Value::Float(f) => assert!((f - 1.5).abs() < 1e-9),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn cap_minus_reduction() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let rows = run_cap_with_project(
            &exe,
            &["d"],
            vec![("d".into(), "minus".into(), 100.0)],
            vec![(
                "d".into(),
                vec![Value::Int(1), Value::Int(2), Value::Int(3)],
            )],
        );
        assert_eq!(rows[0].get("d").unwrap(), &Value::Int(94));
    }

    #[test]
    fn cap_mult_reduction() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let rows = run_cap_with_project(
            &exe,
            &["p"],
            vec![("p".into(), "mult".into(), 1.0)],
            vec![(
                "p".into(),
                vec![Value::Int(2), Value::Int(3), Value::Int(4)],
            )],
        );
        assert_eq!(rows[0].get("p").unwrap(), &Value::Int(24));
    }

    #[test]
    fn cap_div_reduction_promotes_to_float() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let rows = run_cap_with_project(
            &exe,
            &["q"],
            vec![("q".into(), "div".into(), 100.0)],
            vec![("q".into(), vec![Value::Int(2), Value::Int(2)])],
        );
        match rows[0].get("q").unwrap() {
            Value::Float(f) => assert!((f - 25.0).abs() < 1e-9),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn cap_div_zero_skips() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let rows = run_cap_with_project(
            &exe,
            &["q"],
            vec![("q".into(), "div".into(), 100.0)],
            vec![("q".into(), vec![Value::Int(0), Value::Int(2)])],
        );
        match rows[0].get("q").unwrap() {
            Value::Float(f) => assert!((f - 50.0).abs() < 1e-9),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn cap_min_max_reductions() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let rows = run_cap_with_project(
            &exe,
            &["m"],
            vec![("m".into(), "min".into(), 100.0)],
            vec![(
                "m".into(),
                vec![Value::Int(5), Value::Int(2), Value::Int(8)],
            )],
        );
        assert_eq!(rows[0].get("m").unwrap(), &Value::Int(2));

        let rows = run_cap_with_project(
            &exe,
            &["m"],
            vec![("m".into(), "max".into(), 0.0)],
            vec![(
                "m".into(),
                vec![Value::Int(5), Value::Int(2), Value::Int(8)],
            )],
        );
        assert_eq!(rows[0].get("m").unwrap(), &Value::Int(8));
    }

    #[test]
    fn cap_addall_keeps_full_list() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let rows = run_cap_with_project(
            &exe,
            &["a"],
            vec![("a".into(), "addAll".into(), 0.0)],
            vec![("a".into(), vec![Value::Int(1), Value::Int(2)])],
        );
        match rows[0].get("a").unwrap() {
            Value::List(items) => assert_eq!(items.len(), 2),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn cap_assign_keeps_full_list() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let rows = run_cap_with_project(
            &exe,
            &["a"],
            vec![("a".into(), "assign".into(), 0.0)],
            vec![("a".into(), vec![Value::Int(99)])],
        );
        match rows[0].get("a").unwrap() {
            Value::List(items) => assert_eq!(items.len(), 1),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn cap_and_or_boolean_reductions() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let rows = run_cap_with_project(
            &exe,
            &["b"],
            vec![("b".into(), "and".into(), 1.0)],
            vec![(
                "b".into(),
                vec![Value::Bool(true), Value::Bool(true), Value::Int(1)],
            )],
        );
        assert_eq!(rows[0].get("b").unwrap(), &Value::Bool(true));

        let rows = run_cap_with_project(
            &exe,
            &["b"],
            vec![("b".into(), "and".into(), 1.0)],
            vec![("b".into(), vec![Value::Bool(false), Value::Bool(true)])],
        );
        assert_eq!(rows[0].get("b").unwrap(), &Value::Bool(false));

        let rows = run_cap_with_project(
            &exe,
            &["b"],
            vec![("b".into(), "or".into(), 0.0)],
            vec![("b".into(), vec![Value::Bool(false), Value::Int(0)])],
        );
        assert_eq!(rows[0].get("b").unwrap(), &Value::Bool(false));

        let rows = run_cap_with_project(
            &exe,
            &["b"],
            vec![("b".into(), "or".into(), 0.0)],
            vec![("b".into(), vec![Value::Int(0), Value::Bool(true)])],
        );
        assert_eq!(rows[0].get("b").unwrap(), &Value::Bool(true));
    }

    #[test]
    fn cap_unknown_reduction_falls_to_sum() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let rows = run_cap_with_project(
            &exe,
            &["u"],
            vec![("u".into(), "wat".into(), 10.0)],
            vec![(
                "u".into(),
                vec![Value::Int(1), Value::Int(2), Value::Int(3)],
            )],
        );
        assert_eq!(rows[0].get("u").unwrap(), &Value::Int(16));
    }

    #[test]
    fn cap_multi_key_yields_combined_map() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        exe.gremlin_side_effects
            .borrow_mut()
            .insert("__gremlin_agg_a".into(), Value::List(vec![Value::Int(1)]));
        exe.gremlin_side_effects
            .borrow_mut()
            .insert("__gremlin_agg_b".into(), Value::List(vec![Value::Int(2)]));
        let rows = run_cap(&exe, &["a", "b"], vec![]);
        // Combined map column "a_b" should exist
        let combined = rows[0].get("a_b").unwrap();
        match combined {
            Value::Map(m) => {
                assert_eq!(m.len(), 2);
                assert!(m.contains_key("a"));
                assert!(m.contains_key("b"));
            }
            other => panic!("expected combined Map, got {other:?}"),
        }
    }

    #[test]
    fn cap_with_stored_map_returns_it() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let mut m: indexmap::IndexMap<Box<str>, Value> = indexmap::IndexMap::new();
        m.insert("k".into(), Value::Int(7));
        exe.gremlin_side_effects
            .borrow_mut()
            .insert("__gremlin_agg_x".into(), Value::Map(m));
        let rows = run_cap(&exe, &["x"], vec![]);
        match rows[0].get("x").unwrap() {
            Value::Map(m) => assert_eq!(m.get("k"), Some(&Value::Int(7))),
            other => panic!("expected Map, got {other:?}"),
        }
    }
}
