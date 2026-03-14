//! FOREACH operator implementation.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_foreach(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::ForEach {
            variable,
            list_expr,
            body,
            child,
        } = op
        else {
            debug_assert!(false, "expected ForEach operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected ForEach operator".into(),
            }));
        };

        // Execute the child pipeline to get input rows.
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;

            // Evaluate the list expression in the context of this row.
            let ctx = self.build_filter_ctx(cx.graph, &row)?;
            let list_val = self.eval_with_storage(cx.graph, list_expr, &row, &ctx)?;

            match list_val {
                Value::List(items) => {
                    for item in items {
                        cx.budget.check_or_err()?;

                        // Build a seed row: clone the current row and bind the loop variable.
                        let mut seed_row = row.clone();
                        seed_row.insert(variable.0.clone(), item);

                        // Inject the seed row so that exec_single_row (the leaf of
                        // the body sub-plan) returns it instead of an empty row.
                        self.foreach_seed.borrow_mut().replace(vec![seed_row]);

                        // Execute the body sub-plan. The SingleRow leaf will pick
                        // up the seed via foreach_seed, making the loop variable
                        // and all outer bindings available to mutation operators.
                        self.exec_operator(cx, body)?;

                        // Clear the seed in case it wasn't consumed (shouldn't happen).
                        self.foreach_seed.borrow_mut().take();
                    }
                }
                Value::Null => {
                    // FOREACH over null is a no-op (same as empty list).
                }
                other => {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "List for FOREACH".to_string(),
                        actual: other.type_name().to_string(),
                    }));
                }
            }

            // The output row is the original row WITHOUT the loop variable.
            // The variable must not escape the FOREACH scope.
            out.push(row);
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::ast::{Expr, Identifier, Label};
    use crate::executor::budget::ExecutionBudget;
    use crate::executor::engine::QueryExecutor;
    use crate::parser::parse_query;
    use crate::planner::planner::QueryPlanner;
    use crate::planner::{CreatePattern, Operator};
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use crate::types::Value;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    /// Helper: parse, plan, and execute a query against the given store.
    fn run_query(store: &InMemoryGraphStore, query: &str) -> crate::executor::engine::QueryResult {
        let ast = parse_query(query).expect("parse error");
        let planner = QueryPlanner::new(store, "g");
        let plan = planner.plan(&ast).expect("plan error");
        let executor = QueryExecutor::new(store);
        let mut budget = ExecutionBudget::unlimited();
        executor
            .execute("g", &plan, &mut budget)
            .expect("exec error")
    }

    #[test]
    fn foreach_creates_nodes_from_integer_list() {
        let store = make_store();
        let result = run_query(&store, "FOREACH (x IN [1,2,3] | CREATE (:N {val: x}))");
        assert_eq!(result.stats.nodes_created, 3);
        let ids = store.find_nodes_by_label("g", "N").unwrap();
        assert_eq!(ids.len(), 3);
        // Verify the values are 1, 2, 3.
        let mut vals: Vec<i64> = ids
            .iter()
            .map(|id| {
                let node = store.get_node("g", *id).unwrap().unwrap();
                match node.properties.get("val") {
                    Some(Value::Int(v)) => *v,
                    other => panic!("expected Int, got {:?}", other),
                }
            })
            .collect();
        vals.sort();
        assert_eq!(vals, vec![1, 2, 3]);
    }

    #[test]
    fn foreach_creates_named_nodes_from_string_list() {
        let store = make_store();
        let result = run_query(
            &store,
            "FOREACH (name IN ['a','b'] | CREATE (:T {name: name}))",
        );
        assert_eq!(result.stats.nodes_created, 2);
        let ids = store.find_nodes_by_label("g", "T").unwrap();
        assert_eq!(ids.len(), 2);
        let mut names: Vec<String> = ids
            .iter()
            .map(|id| {
                let node = store.get_node("g", *id).unwrap().unwrap();
                match node.properties.get("name") {
                    Some(Value::Str(s)) => s.to_string(),
                    other => panic!("expected Str, got {:?}", other),
                }
            })
            .collect();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn foreach_with_empty_list_is_noop() {
        let store = make_store();
        let result = run_query(&store, "FOREACH (x IN [] | CREATE (:N {val: x}))");
        assert_eq!(result.stats.nodes_created, 0);
        let ids = store.find_nodes_by_label("g", "N").unwrap();
        assert_eq!(ids.len(), 0);
    }

    #[test]
    fn foreach_variable_not_visible_after_block() {
        // After FOREACH, referencing the loop variable should fail.
        // We test this at the execution level: FOREACH followed by RETURN x should error.
        let store = make_store();
        let query = "FOREACH (x IN [1] | CREATE (:N {val: x})) RETURN x";
        let ast = parse_query(query).expect("parse error");
        let planner = QueryPlanner::new(&store, "g");
        let plan_result = planner.plan(&ast);
        // The planner/validator or executor should reject this because x is not in scope.
        // It may fail at plan time (undefined variable) or at execution time.
        // Either way, it should not succeed.
        if let Ok(plan) = plan_result {
            let executor = QueryExecutor::new(&store);
            let mut budget = ExecutionBudget::unlimited();
            let exec_result = executor.execute("g", &plan, &mut budget);
            assert!(
                exec_result.is_err(),
                "referencing FOREACH variable after the block should fail"
            );
        }
        // If plan_result is Err, that's also correct — variable is undefined.
    }

    #[test]
    fn foreach_operator_level_creates_nodes() {
        // Test at the operator level (no parser).
        let store = make_store();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::ForEach {
            variable: Identifier::new("x"),
            list_expr: Expr::List(vec![Expr::Integer(10), Expr::Integer(20)]),
            body: Box::new(Operator::Create {
                patterns: vec![CreatePattern {
                    nodes: vec![(
                        Some(Identifier::new("n")),
                        vec![Label::new("N")],
                        vec![("val".to_string(), Expr::Variable(Identifier::new("x")))],
                    )],
                    edges: vec![],
                }],
                child: Some(Box::new(Operator::SingleRow)),
            }),
            child: Box::new(Operator::SingleRow),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 2);

        // Verify the output rows do NOT contain the variable 'x'.
        for row in &result.rows {
            assert!(
                !row.contains_key("x"),
                "FOREACH variable should not be visible in output rows"
            );
        }
    }

    #[test]
    fn foreach_preserves_outer_row_context() {
        // MATCH (a:Start) FOREACH (x IN [1,2] | CREATE (:N {val: x})) RETURN a
        // The FOREACH should see the matched node, and the RETURN should still see 'a'.
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("root".into()));
        store
            .create_node("g", vec!["Start".to_string()], props)
            .unwrap();

        let result = run_query(
            &store,
            "MATCH (a:Start) FOREACH (x IN [1,2] | CREATE (:N {val: x})) RETURN a",
        );

        // 2 nodes created by FOREACH.
        assert_eq!(result.stats.nodes_created, 2);
        // 1 row returned (the matched node).
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("a"));
    }

    #[test]
    fn foreach_null_list_is_noop() {
        // FOREACH over null should be a no-op (same as empty list).
        let store = make_store();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::ForEach {
            variable: Identifier::new("x"),
            list_expr: Expr::Null,
            body: Box::new(Operator::Create {
                patterns: vec![CreatePattern {
                    nodes: vec![(Some(Identifier::new("n")), vec![Label::new("N")], vec![])],
                    edges: vec![],
                }],
                child: Some(Box::new(Operator::SingleRow)),
            }),
            child: Box::new(Operator::SingleRow),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 0);
    }
}
