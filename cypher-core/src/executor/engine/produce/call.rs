//! CALL procedure operator: exec_call and built-in procedure dispatch.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::helpers::inject_parameters;
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
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
}
