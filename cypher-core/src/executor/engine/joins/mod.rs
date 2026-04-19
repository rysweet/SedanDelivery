//! Join operators, grouped by algorithm.
//!
//! - [`single_row`] — `SingleRow` identity source (and FOREACH seed replay).
//! - [`nested_loop`] — correlated nested-loop joins (`Apply`, `LeftOuterApply`).
//!
//! All `exec_*` methods are `pub(crate)` inherent methods on `QueryExecutor`
//! and remain reachable from the engine dispatch layer unchanged.

mod nested_loop;
mod single_row;

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::ast::{Direction, Identifier, Label, RelType};
    use crate::executor::budget::ExecutionBudget;
    use crate::executor::engine::{ExecContext, MutationStats, QueryExecutor};
    use crate::planner::Operator;
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use crate::types::Value;

    use indexmap::IndexMap;
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
        let mut cx = ExecContext {
            graph: "g",
            budget: &mut budget,
            outer_rows: &[],
            stats: &mut stats,
        };

        let result = executor
            .exec_single_row(&mut cx, &Operator::SingleRow)
            .unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn single_row_keeps_parameters_out_of_rows() {
        let store = make_store();
        let mut params = HashMap::new();
        params.insert("$name".to_string(), Value::Str("Alice".into()));
        let registry = crate::procedure::ProcedureRegistry::new();
        let executor = QueryExecutor::with_context(&store, &registry, params);

        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let mut cx = ExecContext {
            graph: "g",
            budget: &mut budget,
            outer_rows: &[],
            stats: &mut stats,
        };
        let result = executor
            .exec_single_row(&mut cx, &Operator::SingleRow)
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].get("$name"), None);
    }

    #[test]
    fn left_outer_apply_with_matching_right() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "KNOWS".to_string(), IndexMap::new())
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
            .create_node("g", vec!["Person".to_string()], IndexMap::new())
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
            .create_node("g", vec!["A".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_node("g", vec!["B".to_string()], IndexMap::new())
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
            .create_node("g", vec!["A".to_string()], IndexMap::new())
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
            .create_node("g", vec!["X".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_node("g", vec!["X".to_string()], IndexMap::new())
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
