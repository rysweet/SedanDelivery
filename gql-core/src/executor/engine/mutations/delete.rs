//! DELETE and DETACH DELETE operator implementations.

use crate::error::{ExecutionError, GqlError, StorageError};
use crate::planner::Operator;
use crate::storage::{EdgeId, NodeId};
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_delete(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Delete {
            exprs,
            detach,
            child,
        } = op
        else {
            debug_assert!(false, "expected Delete operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Delete operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, &row)?;

            // Two-pass delete: collect all edges and nodes first so that
            // paths sharing nodes don't fail with "node has relationships"
            // when one path's edges are deleted but another path's edges
            // are not yet deleted.
            let mut edges_to_delete: Vec<EdgeId> = Vec::new();
            let mut nodes_to_delete: Vec<NodeId> = Vec::new();

            for expr in exprs {
                let val = self.eval_with_storage(cx.graph, expr, &row, &ctx)?;
                match val {
                    Value::Node(node_id) => {
                        nodes_to_delete.push(node_id);
                    }
                    Value::Edge(edge_id) => {
                        // Cache rel_type before deleting so type(r) still works.
                        if let Ok(Some(data)) = self.store.get_edge(cx.graph, edge_id) {
                            self.deleted_edge_types
                                .borrow_mut()
                                .insert(edge_id.0, data.rel_type);
                        }
                        edges_to_delete.push(edge_id);
                    }
                    Value::Null => {
                        // Deleting NULL is a no-op in Cypher
                    }
                    // Property map (from properties() step): delete the property
                    Value::Map(ref m) if m.contains_key("~key") && m.contains_key("~element") => {
                        if let (Some(Value::Str(key)), Some(elem)) =
                            (m.get("~key"), m.get("~element"))
                        {
                            match elem {
                                Value::Node(nid) => {
                                    let _ = self.store.remove_node_property(
                                        cx.graph,
                                        *nid,
                                        key.as_ref(),
                                    );
                                }
                                Value::Edge(eid) => {
                                    let _ = self.store.remove_edge_property(
                                        cx.graph,
                                        *eid,
                                        key.as_ref(),
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                    Value::Path { nodes, edges } => {
                        // Cache rel_type before deleting so type(r) still works.
                        for &eid in &edges {
                            if let Ok(Some(data)) = self.store.get_edge(cx.graph, eid) {
                                self.deleted_edge_types
                                    .borrow_mut()
                                    .insert(eid.0, data.rel_type);
                            }
                        }
                        edges_to_delete.extend(edges.iter().copied());
                        nodes_to_delete.extend(nodes.iter().copied());
                    }
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "Node or Relationship for DELETE".to_string(),
                            actual: other.type_name().to_string(),
                        }));
                    }
                }
            }

            // Delete all collected edges first.
            for edge_id in edges_to_delete {
                match self.store.delete_edge(cx.graph, edge_id) {
                    Ok(()) => {
                        cx.stats.edges_deleted += 1;
                    }
                    Err(StorageError::EdgeNotFound { .. }) => {} // already deleted
                    Err(e) => return Err(GqlError::Storage(e)),
                }
            }

            // Then delete all collected nodes.
            for node_id in nodes_to_delete {
                if *detach {
                    // Count and delete remaining edges before deleting node.
                    let out_ids = self.store.out_edge_ids(cx.graph, node_id)?;
                    let in_ids = self.store.in_edge_ids(cx.graph, node_id)?;
                    let mut edge_set: std::collections::HashSet<u64> =
                        out_ids.iter().map(|e| e.0).collect();
                    edge_set.extend(in_ids.iter().map(|e| e.0));
                    // Net out edges that were created in this same query.
                    let mut net_deleted_edges: u64 = 0;
                    for eid in &edge_set {
                        if cx.stats.edges_created_ids.remove(eid) {
                            cx.stats.edges_created = cx.stats.edges_created.saturating_sub(1);
                        } else {
                            net_deleted_edges += 1;
                        }
                    }
                    match self.store.delete_node(cx.graph, node_id, true) {
                        Ok(()) => {
                            // Net out: if node was created in this query, cancel the create.
                            if cx.stats.nodes_created_ids.remove(&node_id.0) {
                                cx.stats.nodes_created = cx.stats.nodes_created.saturating_sub(1);
                            } else {
                                cx.stats.nodes_deleted += 1;
                            }
                            cx.stats.edges_deleted += net_deleted_edges;
                        }
                        Err(StorageError::NodeNotFound { .. }) => {}
                        Err(e) => return Err(GqlError::Storage(e)),
                    }
                } else {
                    match self.store.delete_node(cx.graph, node_id, false) {
                        Ok(()) => {
                            // Net out: if node was created in this query, cancel the create.
                            if cx.stats.nodes_created_ids.remove(&node_id.0) {
                                cx.stats.nodes_created = cx.stats.nodes_created.saturating_sub(1);
                            } else {
                                cx.stats.nodes_deleted += 1;
                            }
                        }
                        Err(StorageError::NodeNotFound { .. }) => {} // already deleted
                        Err(e) => return Err(GqlError::Storage(e)),
                    }
                }
            }
            cx.budget.record_row();
            out.push(row);
        }
        self.invalidate_property_cache();
        // In Gremlin mode, drop() is a void step returning no traversers.
        if self.gremlin_mode {
            return Ok(Vec::new());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Direction, Expr, Identifier, Label, RelType};
    use crate::executor::budget::ExecutionBudget;
    use crate::planner::Operator;
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use std::collections::HashMap;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn delete_detach() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Delete {
            exprs: vec![Expr::Variable(Identifier::new("n"))],
            detach: true,
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_deleted, 2);
    }

    #[test]
    fn delete_null_is_noop() {
        let store = make_store();
        store
            .create_node("g", vec!["P".to_string()], HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Delete {
            exprs: vec![Expr::Null],
            detach: false,
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_deleted, 0);
        // The node should still exist
        let ids = store.find_nodes_by_label("g", "P").unwrap();
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn delete_edge_only() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let eid = store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Delete {
            exprs: vec![Expr::Variable(Identifier::new("r"))],
            detach: false,
            child: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("R")],
                src_alias: Identifier::new("a"),
                rel_alias: Some(Identifier::new("r")),
                dst_alias: Identifier::new("b"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("a"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.edges_deleted, 1);
        assert_eq!(result.stats.nodes_deleted, 0);
        assert!(store.get_edge("g", eid).unwrap().is_none());
        // Nodes should still exist
        assert!(store.get_node("g", a).unwrap().is_some());
        assert!(store.get_node("g", b).unwrap().is_some());
    }

    #[test]
    fn delete_type_mismatch_returns_error() {
        let store = make_store();
        store
            .create_node("g", vec!["P".to_string()], HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        // Try to delete an integer, which should fail
        let plan = Operator::Delete {
            exprs: vec![Expr::Integer(42)],
            detach: false,
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget);
        assert!(result.is_err(), "deleting an integer should be an error");
    }

    #[test]
    fn detach_delete_counts_edges() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["M".to_string()], HashMap::new())
            .unwrap();
        // Create 3 edges from a to b
        for _ in 0..3 {
            store
                .create_edge("g", a, b, "R".to_string(), HashMap::new())
                .unwrap();
        }
        let executor = QueryExecutor::new(&store);
        // Detach delete only node a
        let plan = Operator::Delete {
            exprs: vec![Expr::Variable(Identifier::new("n"))],
            detach: true,
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_deleted, 1);
        assert_eq!(result.stats.edges_deleted, 3);
        // b should still exist
        assert!(store.get_node("g", b).unwrap().is_some());
    }

    #[test]
    fn delete_already_deleted_node_is_noop() {
        let store = make_store();
        let _a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let _b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        // Both have label N, so deleting all N nodes will try to delete both.
        // But if we delete them in sequence, the second deletion should be a no-op.
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Delete {
            exprs: vec![Expr::Variable(Identifier::new("n"))],
            detach: false,
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_deleted, 2);
    }

    #[test]
    fn delete_node_without_detach_with_edges_fails() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["M".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Delete {
            exprs: vec![Expr::Variable(Identifier::new("n"))],
            detach: false,
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget);
        assert!(
            result.is_err(),
            "deleting a node with edges without DETACH should fail"
        );
    }
}
