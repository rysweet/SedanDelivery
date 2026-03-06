//! Join operators: exec_left_outer_apply, exec_apply, exec_single_row.

use crate::error::GqlError;
use crate::executor::budget::ExecutionBudget;
use crate::planner::Operator;
use crate::types::Value;

use super::helpers::{discover_operator_aliases, inject_parameters};
use super::{MutationStats, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_single_row(
        &self,
        _graph: &str,
        _op: &Operator,
        budget: &mut ExecutionBudget,
        _outer_rows: &[Row],
        _stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        budget.record_row();
        let mut row = Row::new();
        inject_parameters(&mut row, &self.parameters);
        Ok(vec![row])
    }

    pub(crate) fn exec_left_outer_apply(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } = op
        else {
            unreachable!()
        };
        let left_rows = self.exec_operator(graph, preserved, budget, outer_rows, stats)?;
        let mut out = Vec::new();

        for left_row in left_rows {
            budget.check_or_err()?;
            let right_rows = self.exec_operator(
                graph,
                optional_pattern,
                budget,
                std::slice::from_ref(&left_row),
                stats,
            )?;

            let compatible_rows: Vec<Row> = right_rows
                .into_iter()
                .filter(|right_row| {
                    for (k, v) in right_row {
                        if k.starts_with("__anon") {
                            continue;
                        }
                        if let Some(left_v) = left_row.get(k) {
                            if left_v != v {
                                return false;
                            }
                        }
                    }
                    true
                })
                .collect();

            if compatible_rows.is_empty() {
                let mut result_row = left_row;
                let null_cols = discover_operator_aliases(optional_pattern);
                for col in null_cols {
                    result_row.entry(col).or_insert(Value::Null);
                }
                budget.record_row();
                out.push(result_row);
            } else {
                for right_row in compatible_rows {
                    budget.check_or_err()?;
                    let mut combined = left_row.clone();
                    for (k, v) in right_row {
                        combined.insert(k, v);
                    }
                    budget.record_row();
                    out.push(combined);
                }
            }
        }
        Ok(out)
    }

    pub(crate) fn exec_apply(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Apply { outer, inner } = op else {
            unreachable!()
        };
        let outer_rows = self.exec_operator(graph, outer, budget, outer_rows, stats)?;
        let mut out = Vec::new();

        for outer_row in &outer_rows {
            budget.check_or_err()?;
            let inner_rows =
                self.exec_operator(graph, inner, budget, std::slice::from_ref(outer_row), stats)?;

            for inner_row in inner_rows {
                budget.check_or_err()?;
                let mut compatible = true;
                for (k, v) in &inner_row {
                    if k.starts_with("__anon") {
                        continue;
                    }
                    if let Some(outer_v) = outer_row.get(k) {
                        if outer_v != v {
                            compatible = false;
                            break;
                        }
                    }
                }
                if !compatible {
                    continue;
                }
                let mut combined = outer_row.clone();
                for (k, v) in inner_row {
                    combined.insert(k, v);
                }
                budget.record_row();
                out.push(combined);
            }
        }
        Ok(out)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Direction, Identifier, Label, RelType};
    use crate::executor::budget::ExecutionBudget;
    use crate::storage::{GraphStore, InMemoryGraphStore};

    use std::collections::HashMap;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn single_row_produces_one_empty_row() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();

        let result = executor
            .exec_single_row("g", &Operator::SingleRow, &mut budget, &[], &mut stats)
            .unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn single_row_includes_parameters() {
        let store = make_store();
        let mut params = HashMap::new();
        params.insert("$name".to_string(), Value::Str("Alice".into()));
        let registry = crate::procedure::ProcedureRegistry::new();
        let executor = QueryExecutor::with_context(&store, &registry, params);

        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let result = executor
            .exec_single_row("g", &Operator::SingleRow, &mut budget, &[], &mut stats)
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].get("$name"), Some(&Value::Str("Alice".into())));
    }

    #[test]
    fn left_outer_apply_with_matching_right() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::LeftOuterApply {
            preserved: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("a"),
            }),
            optional_pattern: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("KNOWS")],
                src_alias: Identifier::new("a"),
                rel_alias: Some(Identifier::new("r")),
                dst_alias: Identifier::new("friend"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("a"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // Person a has outgoing KNOWS to b -> gets combined row
        // Person b has no outgoing KNOWS -> gets null-filled row
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn left_outer_apply_no_match_produces_nulls() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::LeftOuterApply {
            preserved: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("a"),
            }),
            optional_pattern: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("KNOWS")],
                src_alias: Identifier::new("a"),
                rel_alias: None,
                dst_alias: Identifier::new("b"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("a"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].get("b"), Some(&Value::Null));
    }

    #[test]
    fn apply_combines_outer_inner() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["B".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Apply {
            outer: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("a"),
            }),
            inner: Box::new(Operator::NodeScan {
                label: Some(Label::new("B")),
                alias: Identifier::new("b"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("a"));
        assert!(result.rows[0].contains_key("b"));
    }

    #[test]
    fn apply_empty_inner_yields_no_rows() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Apply {
            outer: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("a"),
            }),
            inner: Box::new(Operator::NodeScan {
                label: Some(Label::new("NonExistent")),
                alias: Identifier::new("b"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn apply_incompatible_values_filtered_out() {
        // Create two A nodes and two B nodes — Apply should produce cartesian product
        // where compatible variables match.
        let store = make_store();
        store
            .create_node("g", vec!["X".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["X".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        // Both sides scan "X" with alias "a" — inner should only return rows
        // where a matches the outer a (correlated).
        let plan = Operator::Apply {
            outer: Box::new(Operator::NodeScan {
                label: Some(Label::new("X")),
                alias: Identifier::new("a"),
            }),
            inner: Box::new(Operator::NodeScan {
                label: Some(Label::new("X")),
                alias: Identifier::new("a"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // Each outer row should match exactly one inner row (same node).
        assert_eq!(result.rows.len(), 2);
    }
}
