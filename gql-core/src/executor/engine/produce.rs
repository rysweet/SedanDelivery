//! Output and stream operators: exec_produce_results, exec_call, exec_assemble_path,
//! exec_unwind, exec_union.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::storage::{EdgeId, NodeId};
use crate::types::Value;

use super::helpers::{inject_parameters, RowDedupSet};
use super::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_assemble_path(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::AssemblePath {
            path_alias,
            node_aliases,
            edge_aliases,
            child,
        } = op
        else {
            debug_assert!(false, "expected AssemblePath operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected AssemblePath operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());
        for mut row in input {
            cx.budget.check_or_err()?;

            let mut nodes: Vec<NodeId> = Vec::new();
            let mut edges: Vec<EdgeId> = Vec::new();

            let mut fixed_edges_before_varlen: usize = 0;
            let mut varlen_path_found = false;
            let mut varlen_edge_aliases: Vec<String> = Vec::new();
            for ea in edge_aliases {
                match row.get(&ea.0) {
                    Some(Value::Edge(eid)) => {
                        edges.push(*eid);
                        if !varlen_path_found {
                            fixed_edges_before_varlen += 1;
                        }
                    }
                    Some(Value::Path {
                        nodes: pnodes,
                        edges: pedges,
                    }) => {
                        if !varlen_path_found {
                            for na in node_aliases.iter().take(fixed_edges_before_varlen) {
                                if let Some(Value::Node(nid)) = row.get(&na.0) {
                                    nodes.push(*nid);
                                }
                            }
                            nodes.extend_from_slice(pnodes);
                            varlen_path_found = true;
                        } else {
                            nodes.extend_from_slice(&pnodes[1..]);
                        }
                        edges.extend_from_slice(pedges);
                        varlen_edge_aliases.push(ea.0.clone());
                    }
                    Some(Value::List(items)) => {
                        for item in items {
                            if let Value::Edge(eid) = item {
                                edges.push(*eid);
                            }
                        }
                    }
                    _ => {
                        tracing::debug!("unexpected value type in path edge list, skipping");
                    }
                }
            }

            if !varlen_path_found {
                for na in node_aliases {
                    if let Some(Value::Node(nid)) = row.get(&na.0) {
                        nodes.push(*nid);
                    }
                }
            }

            let path_val = Value::Path { nodes, edges };
            row.insert(path_alias.0.clone(), path_val);

            for ea_name in &varlen_edge_aliases {
                if let Some(Value::Path { edges: pedges, .. }) = row.get(ea_name).cloned() {
                    let edge_list: Vec<Value> = pedges.iter().map(|e| Value::Edge(*e)).collect();
                    row.insert(ea_name.clone(), Value::List(edge_list));
                }
            }

            out.push(row);
        }
        Ok(out)
    }

    pub(crate) fn exec_call(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Call {
            procedure,
            args,
            yield_items,
            yield_star,
            standalone,
            child,
        } = op
        else {
            debug_assert!(false, "expected Call operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Call operator".into(),
            }));
        };
        // Handle built-in procedures that query the store dynamically.
        if crate::procedure::is_builtin_procedure(procedure) {
            return self.exec_builtin_call(
                cx,
                procedure,
                yield_items,
                yield_star,
                standalone,
                child,
            );
        }

        let proc_def = self.registry.get(procedure).ok_or_else(|| {
            GqlError::Execution(ExecutionError::unsupported(format!(
                "ProcedureNotFound: {procedure}"
            )))
        })?;

        let input_rows = if let Some(ch) = child {
            self.exec_operator(cx, ch)?
        } else {
            let mut row = Row::new();
            inject_parameters(&mut row, &self.parameters);
            vec![row]
        };

        let mut out = Vec::with_capacity(input_rows.len());
        for input_row in &input_rows {
            cx.budget.check_or_err()?;

            let ctx = self.build_filter_ctx(cx.graph, input_row)?;
            let mut arg_vals = Vec::with_capacity(args.len());
            for arg_expr in args {
                let val = self.eval_with_storage(cx.graph, arg_expr, input_row, &ctx)?;
                arg_vals.push(val);
            }

            let proc_rows = proc_def.execute(&arg_vals);

            if proc_def.output_columns.is_empty() && child.is_none() {
                // Standalone void procedure: produce no output rows.
            } else if proc_def.output_columns.is_empty() {
                cx.budget.record_row();
                out.push(input_row.clone());
            } else if proc_rows.is_empty() {
                // Procedure returned no rows: drop this input row.
            } else {
                for proc_row in proc_rows {
                    cx.budget.check_or_err()?;
                    let mut new_row = input_row.clone();
                    if !yield_items.is_empty() {
                        for (name, alias) in yield_items {
                            let col_name = alias.as_ref().unwrap_or(name).0.clone();
                            let val = proc_row.get(&name.0).cloned().unwrap_or(Value::Null);
                            new_row.insert(col_name, val);
                        }
                    } else if *standalone || *yield_star {
                        for (k, v) in &proc_row {
                            new_row.insert(k.clone(), v.clone());
                        }
                    }
                    cx.budget.record_row();
                    out.push(new_row);
                }
            }
        }
        Ok(out)
    }

    /// Execute a built-in procedure by querying the store dynamically.
    fn exec_builtin_call(
        &self,
        cx: &mut ExecContext<'_>,
        procedure: &str,
        yield_items: &[(crate::ast::Identifier, Option<crate::ast::Identifier>)],
        yield_star: &bool,
        standalone: &bool,
        child: &Option<Box<Operator>>,
    ) -> Result<Vec<Row>, GqlError> {
        let input_rows = if let Some(ch) = child {
            self.exec_operator(cx, ch)?
        } else {
            let mut row = Row::new();
            inject_parameters(&mut row, &self.parameters);
            vec![row]
        };

        // Generate the procedure result rows from the store.
        let proc_rows = self.execute_builtin_procedure(cx.graph, procedure)?;

        let mut out = Vec::new();
        for input_row in &input_rows {
            if proc_rows.is_empty() {
                // No results: drop this input row.
                continue;
            }
            for proc_row in &proc_rows {
                cx.budget.check_or_err()?;
                let mut new_row = input_row.clone();
                if !yield_items.is_empty() {
                    for (name, alias) in yield_items {
                        let col_name = alias.as_ref().unwrap_or(name).0.clone();
                        let val = proc_row.get(&name.0).cloned().unwrap_or(Value::Null);
                        new_row.insert(col_name, val);
                    }
                } else if *standalone || *yield_star {
                    for (k, v) in proc_row {
                        new_row.insert(k.clone(), v.clone());
                    }
                }
                cx.budget.record_row();
                out.push(new_row);
            }
        }
        Ok(out)
    }

    /// Execute a built-in procedure against the store, returning result rows.
    fn execute_builtin_procedure(
        &self,
        graph: &str,
        procedure: &str,
    ) -> Result<Vec<Row>, GqlError> {
        use std::collections::BTreeSet;

        match procedure {
            "db.labels" => {
                let labels = self.store.all_label_names(graph)?;
                let mut sorted: Vec<String> = labels;
                sorted.sort();
                Ok(sorted
                    .into_iter()
                    .map(|l| {
                        let mut row = Row::new();
                        row.insert("label".to_string(), Value::Str(l.into()));
                        row
                    })
                    .collect())
            }
            "db.relationshipTypes" => {
                // Scan all edges to collect distinct relationship types.
                let edge_ids = self.store.all_edge_ids(graph)?;
                let edges = self.store.get_edges_batch(graph, &edge_ids)?;
                let mut types = BTreeSet::new();
                for edge in edges.into_iter().flatten() {
                    types.insert(edge.rel_type);
                }
                Ok(types
                    .into_iter()
                    .map(|t| {
                        let mut row = Row::new();
                        row.insert("relationshipType".to_string(), Value::Str(t.into()));
                        row
                    })
                    .collect())
            }
            "db.propertyKeys" => {
                // Scan all nodes and edges to collect distinct property keys.
                let mut keys = BTreeSet::new();

                let node_ids = self.store.all_node_ids(graph)?;
                let nodes = self.store.get_nodes_batch(graph, &node_ids)?;
                for node in nodes.into_iter().flatten() {
                    for key in node.properties.keys() {
                        keys.insert(key.clone());
                    }
                }

                let edge_ids = self.store.all_edge_ids(graph)?;
                let edges = self.store.get_edges_batch(graph, &edge_ids)?;
                for edge in edges.into_iter().flatten() {
                    for key in edge.properties.keys() {
                        keys.insert(key.clone());
                    }
                }

                Ok(keys
                    .into_iter()
                    .map(|k| {
                        let mut row = Row::new();
                        row.insert("propertyKey".to_string(), Value::Str(k.into()));
                        row
                    })
                    .collect())
            }
            "dbms.components" => {
                let mut row = Row::new();
                row.insert("name".to_string(), Value::Str("SedanDelivery".into()));
                row.insert(
                    "versions".to_string(),
                    Value::List(vec![Value::Str("1.0.0".into())]),
                );
                row.insert("edition".to_string(), Value::Str("community".into()));
                Ok(vec![row])
            }
            "db.schema.visualization" => {
                // Return a minimal schema visualization: list of labels and relationship types.
                let labels = self.store.all_label_names(graph)?;
                let nodes_list: Vec<Value> =
                    labels.into_iter().map(|l| Value::Str(l.into())).collect();

                let edge_ids = self.store.all_edge_ids(graph)?;
                let edges = self.store.get_edges_batch(graph, &edge_ids)?;
                let mut types = BTreeSet::new();
                for edge in edges.into_iter().flatten() {
                    types.insert(edge.rel_type);
                }
                let rels_list: Vec<Value> =
                    types.into_iter().map(|t| Value::Str(t.into())).collect();

                let mut row = Row::new();
                row.insert("nodes".to_string(), Value::List(nodes_list));
                row.insert("relationships".to_string(), Value::List(rels_list));
                Ok(vec![row])
            }
            _ => Err(GqlError::Execution(ExecutionError::unsupported(format!(
                "ProcedureNotFound: {procedure}"
            )))),
        }
    }

    pub(crate) fn exec_produce_results(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::ProduceResults { columns, child } = op else {
            debug_assert!(false, "expected ProduceResults operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected ProduceResults operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;

        if columns.is_empty() {
            return Ok(vec![]);
        }

        let mut out = Vec::with_capacity(input.len());

        let has_star = columns.iter().any(|(name, _)| name == "*");

        macro_rules! try_eval_streaming {
            ($expr_result:expr, $out:ident) => {
                match $expr_result {
                    Ok(val) => val,
                    Err(e) => {
                        if !$out.is_empty() {
                            *self.streaming_error.borrow_mut() = Some(e);
                            return Ok($out);
                        }
                        return Err(e);
                    }
                }
            };
        }

        for row in &input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, row)?;
            let mut new_row = Row::new();

            if has_star {
                for (col_name, expr) in columns {
                    if col_name == "*" {
                        for (var_name, val) in row {
                            if !var_name.starts_with("__") {
                                new_row.insert(var_name.clone(), val.clone());
                            }
                        }
                    } else {
                        let val = try_eval_streaming!(
                            self.eval_with_storage(cx.graph, expr, row, &ctx),
                            out
                        );
                        new_row.insert(col_name.clone(), val);
                    }
                }
            } else {
                for (col_name, expr) in columns {
                    let val =
                        try_eval_streaming!(self.eval_with_storage(cx.graph, expr, row, &ctx), out);
                    new_row.insert(col_name.clone(), val);
                }
            }
            cx.budget.record_row();
            out.push(new_row);
        }
        Ok(out)
    }

    pub(crate) fn exec_unwind(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Unwind {
            list_expr,
            alias,
            child,
        } = op
        else {
            debug_assert!(false, "expected Unwind operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Unwind operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, &row)?;
            let list_val = self.eval_with_storage(cx.graph, list_expr, &row, &ctx)?;
            match list_val {
                Value::List(items) => {
                    for item in items {
                        cx.budget.check_or_err()?;
                        let mut new_row = row.clone();
                        new_row.insert(alias.0.clone(), item);
                        cx.budget.record_row();
                        out.push(new_row);
                    }
                }
                Value::Null => {
                    // UNWIND null produces zero rows (Cypher semantics)
                }
                other => {
                    // Gremlin unfold: treat Map as list of entries, scalar as single-element list
                    match other {
                        Value::Map(m) => {
                            for (k, v) in m.iter() {
                                cx.budget.check_or_err()?;
                                let mut entry = indexmap::IndexMap::new();
                                entry.insert(k.clone(), v.clone());
                                let mut new_row = row.clone();
                                new_row.insert(alias.0.clone(), Value::Map(entry));
                                cx.budget.record_row();
                                out.push(new_row);
                            }
                        }
                        Value::Str(_) | Value::Int(_) | Value::Float(_) | Value::Bool(_)
                        | Value::Node(_) | Value::Edge(_) => {
                            let mut new_row = row.clone();
                            new_row.insert(alias.0.clone(), other);
                            cx.budget.record_row();
                            out.push(new_row);
                        }
                        _ => {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "List for UNWIND".to_string(),
                                actual: "unknown type".to_string(),
                            }));
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    pub(crate) fn exec_union(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Union { left, right, all } = op else {
            debug_assert!(false, "expected Union operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Union operator".into(),
            }));
        };
        let left_rows = self.exec_operator(cx, left)?;
        let right_rows = self.exec_operator(cx, right)?;

        let mut out = Vec::with_capacity(left_rows.len() + right_rows.len());
        if *all {
            out.extend(left_rows);
            out.extend(right_rows);
        } else {
            let mut seen = RowDedupSet::new();
            for row in left_rows.into_iter().chain(right_rows) {
                cx.budget.check_or_err()?;
                if seen.insert(&row) {
                    out.push(row);
                }
            }
        }
        Ok(out)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, Identifier, Label};
    use crate::executor::budget::ExecutionBudget;
    use crate::storage::{GraphStore, InMemoryGraphStore};

    use std::collections::HashMap;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn produce_results_with_property() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ProduceResults {
            columns: vec![(
                "p.name".to_string(),
                Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("p"))),
                    "name".into(),
                ),
            )],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("p"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0]["p.name"], Value::Str("Alice".into()));
    }

    #[test]
    fn produce_results_empty_columns() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ProduceResults {
            columns: vec![],
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn produce_results_star() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("x".to_string(), Value::Int(1));
        store
            .create_node("g", vec!["N".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ProduceResults {
            columns: vec![("*".to_string(), Expr::Variable(Identifier::new("*")))],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("n"));
    }

    #[test]
    fn unwind_list() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Unwind {
            list_expr: Expr::List(vec![Expr::Integer(1), Expr::Integer(2), Expr::Integer(3)]),
            alias: Identifier::new("x"),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 3);
        let values: Vec<&Value> = result.rows.iter().map(|r| &r["x"]).collect();
        assert_eq!(values, vec![&Value::Int(1), &Value::Int(2), &Value::Int(3)]);
    }

    #[test]
    fn unwind_null_produces_zero_rows() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Unwind {
            list_expr: Expr::Null,
            alias: Identifier::new("x"),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn unwind_empty_list() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Unwind {
            list_expr: Expr::List(vec![]),
            alias: Identifier::new("x"),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn unwind_non_list_is_error() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Unwind {
            list_expr: Expr::Integer(42),
            alias: Identifier::new("x"),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget);
        assert!(result.is_err());
    }

    #[test]
    fn union_all_concatenates() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["Movie".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Union {
            left: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
            right: Box::new(Operator::NodeScan {
                label: Some(Label::new("Movie")),
                alias: Identifier::new("n"),
            }),
            all: true,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn union_deduplicates() {
        let store = make_store();
        store
            .create_node("g", vec!["Both".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Union {
            left: Box::new(Operator::NodeScan {
                label: Some(Label::new("Both")),
                alias: Identifier::new("n"),
            }),
            right: Box::new(Operator::NodeScan {
                label: Some(Label::new("Both")),
                alias: Identifier::new("n"),
            }),
            all: false,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn assemble_path_simple() {
        use crate::parser::parse_query;
        use crate::planner::QueryPlanner;

        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");

        let query = "CREATE p = (a:X)-[r:Y]->(b:Z) RETURN p";
        let ast = parse_query(query).unwrap();
        let plan = planner.plan(&ast).unwrap();
        let mut budget = ExecutionBudget::unlimited();
        let result = QueryExecutor::new(&store)
            .execute("g", &plan, &mut budget)
            .unwrap();

        assert_eq!(result.rows.len(), 1);
        let p = result.rows[0].get("p").expect("should have path p");
        match p {
            Value::Path { nodes, edges } => {
                assert_eq!(nodes.len(), 2);
                assert_eq!(edges.len(), 1);
            }
            other => panic!("expected Value::Path, got {:?}", other),
        }
    }
}
