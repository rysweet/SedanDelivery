//! CREATE operator implementation.

use indexmap::IndexMap;
use std::collections::HashMap;

use crate::ast::Direction;
use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::storage::NodeId;
use crate::types::Value;

use crate::executor::engine::helpers::{eval_prop_map, inject_parameters, undefined_var};
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_create(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Create { patterns, child } = op else {
            debug_assert!(false, "expected Create operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Create operator".into(),
            }));
        };
        // Execute child first (for MATCH ... CREATE patterns).
        let input = if let Some(ch) = child {
            self.exec_operator(cx, ch)?
        } else {
            // No child means standalone CREATE: one row with parameters.
            let mut row = Row::new();
            inject_parameters(&mut row, &self.parameters);
            vec![row]
        };

        let mut out = Vec::with_capacity(input.len());
        for mut row in input {
            cx.budget.check_or_err()?;
            for pattern in patterns {
                // Create nodes first, mapping alias -> NodeId.
                let mut alias_to_id: HashMap<String, NodeId> = HashMap::new();

                for (alias, labels, prop_exprs) in &pattern.nodes {
                    // If the alias is a user-defined variable (not anonymous)
                    // and is already bound to a Node in the row, reuse it
                    // instead of creating a new node.
                    if let Some(a) = alias {
                        if !a.0.starts_with("__anon") {
                            if let Some(Value::Node(existing_id)) = row.get(a.0.as_str()) {
                                alias_to_id.insert(a.0.clone(), *existing_id);
                                continue;
                            }
                        }
                    }
                    let ctx = self.build_filter_ctx(cx.graph, &row)?;
                    let props_raw = eval_prop_map(cx.graph, self, prop_exprs, &row, &ctx)?;
                    // In Cypher, setting a property to null on CREATE
                    // does not store it — filter out null values.
                    let props: IndexMap<String, Value> = props_raw
                        .into_iter()
                        .filter(|(_, v)| !v.is_null())
                        .collect();
                    let label_strings: Vec<String> =
                        labels.iter().map(|l| l.as_str().to_string()).collect();
                    let non_null_prop_keys: Vec<String> = props.keys().cloned().collect();
                    let node_id = self
                        .store
                        .create_node(cx.graph, label_strings.clone(), props)?;
                    cx.stats.nodes_created += 1;
                    cx.stats.nodes_created_ids.insert(node_id.0);
                    cx.stats.labels_seen.extend(label_strings);
                    for k in non_null_prop_keys {
                        cx.stats.props_seen.insert((false, node_id.0, k));
                    }
                    if let Some(a) = alias {
                        alias_to_id.insert(a.0.clone(), node_id);
                        row.insert(a.0.clone(), Value::Node(node_id));
                    }
                }

                // Create edges using the alias map for src/dst lookup.
                for (alias, src_alias, dst_alias, rel_type, direction, prop_exprs) in &pattern.edges
                {
                    let ctx = self.build_filter_ctx(cx.graph, &row)?;
                    // Filter out null-valued properties (same as for nodes).
                    let props: IndexMap<String, Value> =
                        eval_prop_map(cx.graph, self, prop_exprs, &row, &ctx)?
                            .into_iter()
                            .filter(|(_, v)| !v.is_null())
                            .collect();

                    let src_id = alias_to_id
                        .get(&src_alias.0)
                        .copied()
                        .or_else(|| {
                            row.get(&src_alias.0).and_then(|v| match v {
                                Value::Node(id) => Some(*id),
                                // Side-effect bindings store vertex IDs as integers
                                Value::Int(n) => Some(NodeId(*n as u64)),
                                _ => None,
                            })
                        })
                        .ok_or_else(|| undefined_var(&src_alias.0))?;

                    let dst_id = alias_to_id
                        .get(&dst_alias.0)
                        .copied()
                        .or_else(|| {
                            row.get(&dst_alias.0).and_then(|v| match v {
                                Value::Node(id) => Some(*id),
                                // Side-effect bindings store vertex IDs as integers
                                Value::Int(n) => Some(NodeId(*n as u64)),
                                _ => None,
                            })
                        })
                        .ok_or_else(|| undefined_var(&dst_alias.0))?;

                    // Determine actual src/dst based on direction.
                    // In the plan, src_alias is always the "left" node in the
                    // pattern text. For an Incoming arc (a)<-[:R]-(b), we need
                    // to flip so the edge is stored as b -> a.
                    let (actual_src, actual_dst) = match direction {
                        Direction::Incoming => (dst_id, src_id),
                        _ => (src_id, dst_id),
                    };

                    let non_null_edge_keys: Vec<String> = props
                        .iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(k, _)| k.clone())
                        .collect();
                    let edge_id = self.store.create_edge(
                        cx.graph,
                        actual_src,
                        actual_dst,
                        rel_type.as_str().to_string(),
                        props,
                    )?;
                    cx.stats.edges_created += 1;
                    cx.stats.edges_created_ids.insert(edge_id.0);
                    for k in non_null_edge_keys {
                        cx.stats.props_seen.insert((true, edge_id.0, k));
                    }
                    if let Some(a) = alias {
                        row.insert(a.0.clone(), Value::Edge(edge_id));
                    }
                }
            }
            cx.budget.record_row();
            out.push(row);
        }
        self.invalidate_property_cache();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, Identifier, Label};
    use crate::executor::budget::ExecutionBudget;
    use crate::planner::{CreatePattern, Operator};
    use crate::storage::{GraphStore, InMemoryGraphStore};

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn create_node() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![(
                    Some(Identifier::new("n")),
                    vec![Label::new("P")],
                    vec![("name".to_string(), Expr::Str("A".into()))],
                )],
                edges: vec![],
            }],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 1);
    }

    #[test]
    fn create_node_with_multiple_labels() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![(
                    Some(Identifier::new("n")),
                    vec![Label::new("Person"), Label::new("Employee")],
                    vec![],
                )],
                edges: vec![],
            }],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 1);
        let ids = store.find_nodes_by_label("g", "Person").unwrap();
        assert_eq!(ids.len(), 1);
        let ids2 = store.find_nodes_by_label("g", "Employee").unwrap();
        assert_eq!(ids2.len(), 1);
    }

    #[test]
    fn create_node_null_properties_filtered() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![(
                    Some(Identifier::new("n")),
                    vec![Label::new("P")],
                    vec![
                        ("name".to_string(), Expr::Str("Alice".into())),
                        ("age".to_string(), Expr::Null),
                    ],
                )],
                edges: vec![],
            }],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 1);
        let ids = store.find_nodes_by_label("g", "P").unwrap();
        let node = store.get_node("g", ids[0]).unwrap().unwrap();
        assert_eq!(
            node.properties.get("name"),
            Some(&Value::Str("Alice".into()))
        );
        assert!(
            !node.properties.contains_key("age"),
            "null property should be filtered out"
        );
    }

    #[test]
    fn create_node_binds_variable_in_row() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![(Some(Identifier::new("n")), vec![Label::new("P")], vec![])],
                edges: vec![],
            }],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert!(
            matches!(result.rows[0].get("n"), Some(Value::Node(_))),
            "created node should be bound in the output row"
        );
    }

    #[test]
    fn create_edge_between_new_nodes() {
        use crate::ast::RelType;
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![
                    (Some(Identifier::new("a")), vec![Label::new("P")], vec![]),
                    (Some(Identifier::new("b")), vec![Label::new("P")], vec![]),
                ],
                edges: vec![(
                    Some(Identifier::new("r")),
                    Identifier::new("a"),
                    Identifier::new("b"),
                    RelType::new("KNOWS"),
                    Direction::Outgoing,
                    vec![],
                )],
            }],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 2);
        assert_eq!(result.stats.edges_created, 1);
    }

    #[test]
    fn create_edge_with_properties() {
        use crate::ast::RelType;
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![
                    (Some(Identifier::new("a")), vec![Label::new("P")], vec![]),
                    (Some(Identifier::new("b")), vec![Label::new("P")], vec![]),
                ],
                edges: vec![(
                    Some(Identifier::new("r")),
                    Identifier::new("a"),
                    Identifier::new("b"),
                    RelType::new("KNOWS"),
                    Direction::Outgoing,
                    vec![("since".to_string(), Expr::Integer(2020))],
                )],
            }],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.edges_created, 1);
        // Verify the edge has the property
        if let Some(Value::Edge(eid)) = result.rows[0].get("r") {
            let edge = store.get_edge("g", *eid).unwrap().unwrap();
            assert_eq!(edge.properties.get("since"), Some(&Value::Int(2020)));
        } else {
            panic!("expected edge in row");
        }
    }

    #[test]
    fn create_multiple_nodes_in_single_pattern() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![
                    (Some(Identifier::new("a")), vec![Label::new("A")], vec![]),
                    (Some(Identifier::new("b")), vec![Label::new("B")], vec![]),
                    (Some(Identifier::new("c")), vec![Label::new("C")], vec![]),
                ],
                edges: vec![],
            }],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 3);
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("a"));
        assert!(result.rows[0].contains_key("b"));
        assert!(result.rows[0].contains_key("c"));
    }

    #[test]
    fn create_incoming_edge_flips_direction() {
        use crate::ast::RelType;
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![
                    (Some(Identifier::new("a")), vec![Label::new("P")], vec![]),
                    (Some(Identifier::new("b")), vec![Label::new("P")], vec![]),
                ],
                edges: vec![(
                    Some(Identifier::new("r")),
                    Identifier::new("a"),
                    Identifier::new("b"),
                    RelType::new("R"),
                    Direction::Incoming, // a <-[:R]- b, stored as b -> a
                    vec![],
                )],
            }],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.edges_created, 1);
        // Verify the edge goes from b to a (src=b, dst=a)
        let a_id = match result.rows[0].get("a") {
            Some(Value::Node(id)) => *id,
            _ => panic!("expected node a"),
        };
        let b_id = match result.rows[0].get("b") {
            Some(Value::Node(id)) => *id,
            _ => panic!("expected node b"),
        };
        // b should have outgoing edge, a should have incoming
        let out_edges = store.out_edge_ids("g", b_id).unwrap();
        assert_eq!(out_edges.len(), 1);
        let in_edges = store.in_edge_ids("g", a_id).unwrap();
        assert_eq!(in_edges.len(), 1);
    }

    #[test]
    fn create_reuses_existing_bound_node() {
        let store = make_store();
        // Pre-create a node
        let existing = store
            .create_node("g", vec!["P".to_string()], IndexMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        // Scan existing node, then CREATE (n)-[:R]->(m)
        // n is already bound from the scan, so it should be reused
        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![
                    (Some(Identifier::new("n")), vec![Label::new("P")], vec![]),
                    (Some(Identifier::new("m")), vec![Label::new("Q")], vec![]),
                ],
                edges: vec![],
            }],
            child: Some(Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            })),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // n was already bound, so only m is new
        assert_eq!(result.stats.nodes_created, 1);
        // Verify n is still the original node
        assert_eq!(result.rows[0].get("n"), Some(&Value::Node(existing)));
    }
}
