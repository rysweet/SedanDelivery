//! Node scanning operators: `NodeScan`, `IndexScan`, `NodeByIdSeek`.
//!
//! Each operator's executor lives in its own submodule. They all extend the
//! same inherent `impl QueryExecutor` block, and the `pub(crate)` methods
//! remain reachable through `crate::executor::engine::scan::node_scan::*`.

mod by_id_seek;
mod index_scan;
mod node_scan_op;

#[cfg(test)]
mod tests {
    use crate::ast::{Expr, Identifier, Label};
    use crate::executor::budget::ExecutionBudget;
    use crate::executor::engine::MutationStats;
    use crate::executor::engine::{ExecContext, QueryExecutor, Row};
    use crate::planner::Operator;
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use crate::types::Value;
    use indexmap::IndexMap;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn node_scan_all_nodes() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_node("g", vec!["B".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn node_scan_with_label_filter() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_node("g", vec!["Movie".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn node_scan_empty_graph() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn node_scan_nonexistent_label() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: Some(Label::new("NonExistent")),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn node_scan_binds_correct_alias() {
        let store = make_store();
        let nid = store
            .create_node("g", vec!["P".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("mynode"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].get("mynode"), Some(&Value::Node(nid)));
    }

    #[test]
    fn node_scan_budget_enforced_in_inner_loop() {
        use std::time::Duration;

        let store = make_store();
        for _ in 0..3 {
            store
                .create_node("g", vec!["N".to_string()], IndexMap::new())
                .unwrap();
        }

        let outer_rows: Vec<Row> = (0..3)
            .map(|_| {
                let mut r = Row::new();
                r.insert("__dummy__".to_string(), Value::Int(0));
                r
            })
            .collect();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("x"),
        };

        let mut budget = ExecutionBudget::new(Duration::from_secs(60), 2);
        let mut stats = MutationStats::default();
        let mut cx = ExecContext {
            graph: "g",
            budget: &mut budget,
            outer_rows: &outer_rows,
            stats: &mut stats,
        };
        let result = executor.exec_node_scan(&mut cx, &plan);
        assert!(
            result.is_err(),
            "expected budget error for Cartesian product exceeding row limit"
        );
    }

    #[test]
    fn index_scan_finds_nodes() {
        let store = make_store();
        let mut p1 = IndexMap::new();
        p1.insert("age".to_string(), Value::Int(30));
        store
            .create_node("g", vec!["Person".to_string()], p1)
            .unwrap();
        store
            .create_node("g", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::IndexScan {
            label: Label::new("Person"),
            property: "age".to_string(),
            value: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn index_scan_empty_when_no_property() {
        let store = make_store();
        store
            .create_node("g", vec!["P".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::IndexScan {
            label: Label::new("P"),
            property: "nonexistent".to_string(),
            value: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn node_by_id_seek_found() {
        let store = make_store();
        let node_id = store
            .create_node("g", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: crate::ast::Expr::Integer(node_id.0 as i64),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn node_by_id_seek_not_found() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(99999),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn node_by_id_seek_negative_id_errors() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(-1),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget);
        assert!(result.is_err());
    }

    #[test]
    fn node_scan_with_outer_rows_merges_context() {
        let store = make_store();
        let nid = store
            .create_node("g", vec!["P".to_string()], IndexMap::new())
            .unwrap();

        let mut outer = Row::new();
        outer.insert("ctx".to_string(), Value::Int(42));

        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let mut cx = ExecContext {
            graph: "g",
            budget: &mut budget,
            outer_rows: &[outer],
            stats: &mut stats,
        };
        let rows = executor.exec_node_scan(&mut cx, &plan).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("ctx"), Some(&Value::Int(42)));
        assert_eq!(rows[0].get("n"), Some(&Value::Node(nid)));
    }
}
