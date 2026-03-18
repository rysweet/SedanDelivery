//! Gremlin traversal operator implementations.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    /// Execute `Repeat` — run `body` repeatedly, either a fixed number of
    /// `times` or until the `until` predicate produces results.  When `emit`
    /// is true, intermediate results from each iteration are included.
    pub(crate) fn exec_repeat(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Repeat {
            body,
            times,
            until,
            emit,
            child,
        } = op
        else {
            debug_assert!(false, "expected Repeat operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Repeat operator".into(),
            }));
        };

        let mut current = self.exec_operator(cx, child)?;
        let mut collected: Vec<Row> = Vec::new();

        let max_iterations = times.unwrap_or(256);
        for _ in 0..max_iterations {
            cx.budget.check_or_err()?;

            if *emit {
                collected.extend(current.clone());
            }

            // Feed current rows through the body.
            let mut next = Vec::new();
            for row in &current {
                cx.budget.check_or_err()?;
                self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                let body_rows = self.exec_operator(cx, body)?;
                self.foreach_seed.borrow_mut().take();
                next.extend(body_rows);
            }

            if next.is_empty() {
                break;
            }
            current = next;

            // Check the until predicate if one exists.
            if let Some(until_op) = until {
                let mut should_stop = false;
                for row in &current {
                    self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                    let result = self.exec_operator(cx, until_op)?;
                    self.foreach_seed.borrow_mut().take();
                    if !result.is_empty() {
                        should_stop = true;
                        break;
                    }
                }
                if should_stop {
                    break;
                }
            }

            // For fixed-count mode without `until`, the loop counter handles termination.
            if times.is_some() {
                continue;
            }
        }

        collected.extend(current);
        Ok(collected)
    }

    /// Execute `Choose` — evaluate the predicate for each input row and route
    /// to `if_true` or `if_false` accordingly.
    pub(crate) fn exec_choose(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Choose {
            predicate,
            if_true,
            if_false,
            child,
        } = op
        else {
            debug_assert!(false, "expected Choose operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Choose operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;

            // Evaluate predicate: if it produces rows, the condition is true.
            self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
            let pred_result = self.exec_operator(cx, predicate)?;
            self.foreach_seed.borrow_mut().take();

            let branch = if !pred_result.is_empty() {
                if_true
            } else if let Some(f) = if_false {
                f
            } else {
                // No else branch — pass through unchanged.
                out.push(row);
                continue;
            };

            self.foreach_seed.borrow_mut().replace(vec![row]);
            let branch_rows = self.exec_operator(cx, branch)?;
            self.foreach_seed.borrow_mut().take();
            out.extend(branch_rows);
        }
        Ok(out)
    }

    /// Execute `Coalesce` — try each alternative in order, return results from
    /// the first one that produces non-empty output.
    pub(crate) fn exec_coalesce(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Coalesce {
            alternatives,
            child,
        } = op
        else {
            debug_assert!(false, "expected Coalesce operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Coalesce operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;
            let mut matched = false;

            for alt in alternatives {
                self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                let alt_rows = self.exec_operator(cx, alt)?;
                self.foreach_seed.borrow_mut().take();

                if !alt_rows.is_empty() {
                    out.extend(alt_rows);
                    matched = true;
                    break;
                }
            }

            if !matched {
                // No alternative matched — pass through.
                out.push(row);
            }
        }
        Ok(out)
    }

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
    /// Phase 1 stub: executes the child and returns empty results.
    /// Full implementation will integrate with a side-effect accumulator.
    pub(crate) fn exec_cap(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Cap { keys, child } = op else {
            debug_assert!(false, "expected Cap operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Cap operator".into(),
            }));
        };

        // Execute child for any side effects.
        let _child_rows = self.exec_operator(cx, child)?;

        // TODO(gremlin): integrate with side-effect accumulator to return
        // collected values for `keys`. For now, return a single row with
        // Null for each requested key.
        let mut row = Row::new();
        for key in keys {
            row.insert(key.clone(), Value::Null);
        }
        Ok(vec![row])
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::ast::{Direction, Expr, Identifier, Label, RelType};
    use crate::executor::budget::ExecutionBudget;
    use crate::executor::engine::QueryExecutor;
    use crate::planner::Operator;
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use crate::types::Value;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    /// Build a linear chain A->B->C and return the store.
    fn make_chain() -> InMemoryGraphStore {
        let store = make_store();
        let mut pa = HashMap::new();
        pa.insert("name".to_string(), Value::Str("A".into()));
        let a = store.create_node("g", vec!["N".to_string()], pa).unwrap();
        let mut pb = HashMap::new();
        pb.insert("name".to_string(), Value::Str("B".into()));
        let b = store.create_node("g", vec!["N".to_string()], pb).unwrap();
        let mut pc = HashMap::new();
        pc.insert("name".to_string(), Value::Str("C".into()));
        let _c = store.create_node("g", vec!["N".to_string()], pc).unwrap();
        store
            .create_edge("g", a, b, "NEXT".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, _c, "NEXT".to_string(), HashMap::new())
            .unwrap();
        store
    }

    // ── Repeat tests ─────────────────────────────────────────────────────────

    #[test]
    fn repeat_fixed_times_traverses_levels() {
        let store = make_chain();
        let executor = QueryExecutor::new(&store);

        // Scan all :N nodes, then repeat out-expand once.
        // With times=1, each starting node expands one hop outward.
        let plan = Operator::Repeat {
            body: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("n"),
                rel_alias: None,
                dst_alias: Identifier::new("n"),
                child: Box::new(Operator::SingleRow),
            }),
            times: Some(1),
            until: None,
            emit: false,
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // A->B, B->C expand succeeds; C has no outgoing so it stays from scan.
        // Total: 3 rows after one expansion iteration.
        assert_eq!(result.rows.len(), 3);
    }

    #[test]
    fn repeat_with_emit_includes_intermediates() {
        let store = make_chain();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::Repeat {
            body: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("n"),
                rel_alias: None,
                dst_alias: Identifier::new("n"),
                child: Box::new(Operator::SingleRow),
            }),
            times: Some(2),
            until: None,
            emit: true,
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // emit=true collects intermediates: iter0 emits 3 (scan), iter1 emits 2 (expand),
        // plus final 1 (A->B->C). Total should be > 2.
        assert!(
            result.rows.len() > 2,
            "emit=true should produce more rows than without emit, got {}",
            result.rows.len()
        );
    }

    // ── Choose tests ─────────────────────────────────────────────────────────

    #[test]
    fn choose_routes_based_on_predicate() {
        let store = make_store();
        // Create two nodes: one labeled "Yes", one labeled "No".
        let mut py = HashMap::new();
        py.insert("tag".to_string(), Value::Str("yes".into()));
        store.create_node("g", vec!["Yes".to_string()], py).unwrap();
        let mut pn = HashMap::new();
        pn.insert("tag".to_string(), Value::Str("no".into()));
        store.create_node("g", vec!["No".to_string()], pn).unwrap();

        let executor = QueryExecutor::new(&store);

        // Predicate: scan for :Yes label (rows with a Yes node produce results).
        // if_true: project tag as "matched"
        // if_false: project tag as "unmatched"
        let plan = Operator::Choose {
            predicate: Box::new(Operator::NodeScan {
                label: Some(Label::new("Yes")),
                alias: Identifier::new("check"),
            }),
            if_true: Box::new(Operator::Project {
                items: vec![(
                    Expr::Str("routed_true".into()),
                    Some(Identifier::new("result")),
                )],
                child: Box::new(Operator::SingleRow),
            }),
            if_false: Some(Box::new(Operator::Project {
                items: vec![(
                    Expr::Str("routed_false".into()),
                    Some(Identifier::new("result")),
                )],
                child: Box::new(Operator::SingleRow),
            })),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // 2 input nodes -> 2 output rows
        assert_eq!(result.rows.len(), 2);
        let results: Vec<_> = result.rows.iter().filter_map(|r| r.get("result")).collect();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn choose_without_else_passes_through() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);

        // Predicate that never matches (scan for label "NOPE").
        // No if_false -> row passes through unchanged.
        let plan = Operator::Choose {
            predicate: Box::new(Operator::NodeScan {
                label: Some(Label::new("NOPE")),
                alias: Identifier::new("check"),
            }),
            if_true: Box::new(Operator::Project {
                items: vec![(
                    Expr::Str("should_not_appear".into()),
                    Some(Identifier::new("x")),
                )],
                child: Box::new(Operator::SingleRow),
            }),
            if_false: None,
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        // Should have "n" (the original scan) but not "x".
        assert!(result.rows[0].contains_key("n"));
        assert!(!result.rows[0].contains_key("x"));
    }

    // ── Coalesce tests ───────────────────────────────────────────────────────

    #[test]
    fn coalesce_returns_first_nonempty() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);

        // First alternative: scan for "NOPE" (empty).
        // Second alternative: project a literal (non-empty).
        let plan = Operator::Coalesce {
            alternatives: vec![
                Operator::NodeScan {
                    label: Some(Label::new("NOPE")),
                    alias: Identifier::new("miss"),
                },
                Operator::Project {
                    items: vec![(Expr::Str("fallback".into()), Some(Identifier::new("val")))],
                    child: Box::new(Operator::SingleRow),
                },
            ],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].get("val"),
            Some(&Value::Str("fallback".into()))
        );
    }

    #[test]
    fn coalesce_uses_first_match_when_multiple_succeed() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);

        let plan = Operator::Coalesce {
            alternatives: vec![
                Operator::Project {
                    items: vec![(Expr::Str("first".into()), Some(Identifier::new("val")))],
                    child: Box::new(Operator::SingleRow),
                },
                Operator::Project {
                    items: vec![(Expr::Str("second".into()), Some(Identifier::new("val")))],
                    child: Box::new(Operator::SingleRow),
                },
            ],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].get("val"), Some(&Value::Str("first".into())));
    }

    // ── GremlinSideEffect tests ──────────────────────────────────────────────

    #[test]
    fn side_effect_passes_input_through() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("root".into()));
        store
            .create_node("g", vec!["A".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);

        // Side effect: project something (result is discarded).
        // Input rows should pass through unchanged.
        let plan = Operator::GremlinSideEffect {
            effect: Box::new(Operator::Project {
                items: vec![(
                    Expr::Str("ignored".into()),
                    Some(Identifier::new("discard")),
                )],
                child: Box::new(Operator::SingleRow),
            }),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("n"));
        assert!(!result.rows[0].contains_key("discard"));
    }

    // ── Cap tests ────────────────────────────────────────────────────────────

    #[test]
    fn cap_returns_null_stub_for_keys() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::Cap {
            keys: vec!["x".to_string(), "y".to_string()],
            child: Box::new(Operator::SingleRow),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].get("x"), Some(&Value::Null));
        assert_eq!(result.rows[0].get("y"), Some(&Value::Null));
    }
}
