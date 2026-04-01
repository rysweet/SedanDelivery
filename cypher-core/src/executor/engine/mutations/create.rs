//! CREATE operator implementation.

use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

use crate::ast::{Direction, Expr};
use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::storage::NodeId;
use crate::types::Value;

use crate::executor::engine::helpers::{eval_prop_map, inject_parameters, undefined_var};
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

struct PendingCreateNode {
    alias: Option<String>,
    labels: Vec<String>,
    non_null_prop_keys: Vec<String>,
    props: IndexMap<String, Value>,
}

struct PendingCreateEdge {
    alias: Option<String>,
    non_null_prop_keys: Vec<String>,
    src: NodeId,
    dst: NodeId,
    rel_type: String,
    props: IndexMap<String, Value>,
}

fn expr_references_any_alias(expr: &Expr, aliases: &HashSet<String>) -> bool {
    match expr {
        Expr::Variable(id) => aliases.contains(id.0.as_str()),
        Expr::Property(obj, key) => {
            let _ = key;
            expr_references_any_alias(obj, aliases)
        }
        Expr::BinaryOp { left, right, .. } => {
            expr_references_any_alias(left, aliases) || expr_references_any_alias(right, aliases)
        }
        Expr::UnaryOp { expr, .. }
        | Expr::IsNull(expr)
        | Expr::IsNotNull(expr)
        | Expr::HasLabel(expr, _) => expr_references_any_alias(expr, aliases),
        Expr::FunctionCall { args, .. } | Expr::List(args) => args
            .iter()
            .any(|arg| expr_references_any_alias(arg, aliases)),
        Expr::MapLiteral(entries) => entries
            .iter()
            .any(|(_, value)| expr_references_any_alias(value, aliases)),
        Expr::Case {
            subject,
            alternatives,
            default,
        } => {
            subject
                .as_deref()
                .is_some_and(|expr| expr_references_any_alias(expr, aliases))
                || alternatives.iter().any(|(when, then)| {
                    expr_references_any_alias(when, aliases)
                        || expr_references_any_alias(then, aliases)
                })
                || default
                    .as_deref()
                    .is_some_and(|expr| expr_references_any_alias(expr, aliases))
        }
        Expr::Subscript(list, index) => {
            expr_references_any_alias(list, aliases) || expr_references_any_alias(index, aliases)
        }
        Expr::Slice(list, start, end) => {
            expr_references_any_alias(list, aliases)
                || start
                    .as_deref()
                    .is_some_and(|expr| expr_references_any_alias(expr, aliases))
                || end
                    .as_deref()
                    .is_some_and(|expr| expr_references_any_alias(expr, aliases))
        }
        Expr::ListComp {
            variable,
            list,
            filter,
            map,
        } => {
            if expr_references_any_alias(list, aliases) {
                return true;
            }
            let scoped_aliases: HashSet<String> = aliases
                .iter()
                .filter(|alias| alias.as_str() != variable.0.as_str())
                .cloned()
                .collect();
            filter
                .as_deref()
                .is_some_and(|expr| expr_references_any_alias(expr, &scoped_aliases))
                || expr_references_any_alias(map, &scoped_aliases)
        }
        Expr::Reduce {
            accumulator,
            initial,
            variable,
            list,
            expression,
        } => {
            if expr_references_any_alias(initial, aliases)
                || expr_references_any_alias(list, aliases)
            {
                return true;
            }
            let scoped_aliases: HashSet<String> = aliases
                .iter()
                .filter(|alias| {
                    alias.as_str() != accumulator.0.as_str()
                        && alias.as_str() != variable.0.as_str()
                })
                .cloned()
                .collect();
            expr_references_any_alias(expression, &scoped_aliases)
        }
        Expr::ExistsSubquery {
            where_clause,
            full_query,
            ..
        } => {
            where_clause
                .as_deref()
                .is_some_and(|expr| expr_references_any_alias(expr, aliases))
                || full_query.is_some()
        }
        Expr::PatternExpr(_) => !aliases.is_empty(),
        Expr::Integer(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Bool(_)
        | Expr::Null
        | Expr::Parameter(_) => false,
    }
}

fn prop_exprs_reference_any_alias(
    prop_exprs: &[(String, Expr)],
    aliases: &HashSet<String>,
) -> bool {
    prop_exprs
        .iter()
        .any(|(_, expr)| expr_references_any_alias(expr, aliases))
}

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
                let mut pending_nodes: Vec<PendingCreateNode> = Vec::new();
                let mut pending_aliases: HashSet<String> = HashSet::new();

                let mut flush_pending_nodes = |row: &mut Row,
                                               alias_to_id: &mut HashMap<String, NodeId>,
                                               pending_nodes: &mut Vec<PendingCreateNode>,
                                               pending_aliases: &mut HashSet<String>|
                 -> Result<(), GqlError> {
                    if pending_nodes.is_empty() {
                        return Ok(());
                    }

                    let pending = std::mem::take(pending_nodes);
                    let mut batch = Vec::with_capacity(pending.len());
                    let mut metadata = Vec::with_capacity(pending.len());
                    for pending_node in pending {
                        batch.push((pending_node.labels.clone(), pending_node.props));
                        metadata.push((
                            pending_node.alias,
                            pending_node.labels,
                            pending_node.non_null_prop_keys,
                        ));
                    }

                    let created_node_ids = self.store.batch_create_nodes(cx.graph, batch)?;
                    for (node_id, (alias, labels, non_null_prop_keys)) in
                        created_node_ids.into_iter().zip(metadata.into_iter())
                    {
                        cx.stats.nodes_created += 1;
                        cx.stats.nodes_created_ids.insert(node_id.0);
                        cx.stats.labels_seen.extend(labels);
                        for key in non_null_prop_keys {
                            cx.stats.props_seen.insert((false, node_id.0, key));
                        }
                        if let Some(alias) = alias {
                            alias_to_id.insert(alias.clone(), node_id);
                            row.insert(alias, Value::Node(node_id));
                        }
                    }
                    pending_aliases.clear();
                    Ok(())
                };

                for (alias, labels, prop_exprs) in &pattern.nodes {
                    // If the alias is a user-defined variable (not anonymous)
                    // and is already bound to a Node in the row, reuse it
                    // instead of creating a new node.
                    if let Some(a) = alias {
                        if !a.0.starts_with("__anon") {
                            if pending_aliases.contains(a.0.as_str()) {
                                flush_pending_nodes(
                                    &mut row,
                                    &mut alias_to_id,
                                    &mut pending_nodes,
                                    &mut pending_aliases,
                                )?;
                            }
                            if let Some(Value::Node(existing_id)) = row.get(a.0.as_str()) {
                                alias_to_id.insert(a.0.clone(), *existing_id);
                                continue;
                            }
                        }
                    }
                    if prop_exprs_reference_any_alias(prop_exprs, &pending_aliases) {
                        flush_pending_nodes(
                            &mut row,
                            &mut alias_to_id,
                            &mut pending_nodes,
                            &mut pending_aliases,
                        )?;
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
                    if let Some(alias) = alias.as_ref().map(|a| a.0.clone()) {
                        pending_aliases.insert(alias.clone());
                        pending_nodes.push(PendingCreateNode {
                            alias: Some(alias),
                            labels: label_strings,
                            non_null_prop_keys: props.keys().cloned().collect::<Vec<_>>(),
                            props,
                        });
                    } else {
                        pending_nodes.push(PendingCreateNode {
                            alias: None,
                            labels: label_strings,
                            non_null_prop_keys: props.keys().cloned().collect::<Vec<_>>(),
                            props,
                        });
                    }
                }

                flush_pending_nodes(
                    &mut row,
                    &mut alias_to_id,
                    &mut pending_nodes,
                    &mut pending_aliases,
                )?;

                // Create edges using the alias map for src/dst lookup.
                let mut pending_edges: Vec<PendingCreateEdge> = Vec::new();
                let mut pending_edge_aliases: HashSet<String> = HashSet::new();
                let mut flush_pending_edges = |row: &mut Row,
                                               pending_edges: &mut Vec<PendingCreateEdge>,
                                               pending_edge_aliases: &mut HashSet<String>|
                 -> Result<(), GqlError> {
                    if pending_edges.is_empty() {
                        return Ok(());
                    }

                    let pending = std::mem::take(pending_edges);
                    let mut batch = Vec::with_capacity(pending.len());
                    let mut metadata = Vec::with_capacity(pending.len());
                    for pending_edge in pending {
                        batch.push((
                            pending_edge.src,
                            pending_edge.dst,
                            pending_edge.rel_type,
                            pending_edge.props,
                        ));
                        metadata.push((pending_edge.alias, pending_edge.non_null_prop_keys));
                    }

                    let created_edge_ids = self.store.batch_create_edges(cx.graph, batch)?;
                    for (edge_id, (alias, non_null_edge_keys)) in
                        created_edge_ids.into_iter().zip(metadata.into_iter())
                    {
                        cx.stats.edges_created += 1;
                        cx.stats.edges_created_ids.insert(edge_id.0);
                        for key in non_null_edge_keys {
                            cx.stats.props_seen.insert((true, edge_id.0, key));
                        }
                        if let Some(alias) = alias {
                            row.insert(alias, Value::Edge(edge_id));
                        }
                    }
                    pending_edge_aliases.clear();
                    Ok(())
                };
                for (alias, src_alias, dst_alias, rel_type, direction, prop_exprs) in &pattern.edges
                {
                    if prop_exprs_reference_any_alias(prop_exprs, &pending_edge_aliases) {
                        flush_pending_edges(
                            &mut row,
                            &mut pending_edges,
                            &mut pending_edge_aliases,
                        )?;
                    }
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

                    let alias = alias.as_ref().map(|a| a.0.clone());
                    if let Some(alias_name) = alias.as_ref() {
                        pending_edge_aliases.insert(alias_name.clone());
                    }
                    pending_edges.push(PendingCreateEdge {
                        alias,
                        non_null_prop_keys: props.keys().cloned().collect::<Vec<_>>(),
                        src: actual_src,
                        dst: actual_dst,
                        rel_type: rel_type.as_str().to_string(),
                        props,
                    });
                }

                flush_pending_edges(&mut row, &mut pending_edges, &mut pending_edge_aliases)?;
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
    fn parameterized_unwind_create_does_not_retain_batch_parameter_in_rows() {
        let store = make_store();
        let mut params = std::collections::HashMap::new();

        let mut first = IndexMap::new();
        first.insert("name".into(), Value::Str("Alice".into()));
        let mut second = IndexMap::new();
        second.insert("name".into(), Value::Str("Bob".into()));

        params.insert(
            "resources".to_string(),
            Value::List(vec![Value::Map(first), Value::Map(second)]),
        );
        params.insert("tenant".to_string(), Value::Str("acme".into()));

        let registry = crate::procedure::ProcedureRegistry::new();
        let executor = QueryExecutor::with_context(&store, &registry, params);
        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![(
                    Some(Identifier::new("n")),
                    vec![Label::new("Person")],
                    vec![
                        (
                            "name".to_string(),
                            Expr::Property(
                                Box::new(Expr::Variable(Identifier::new("resource"))),
                                "name".into(),
                            ),
                        ),
                        ("tenant".to_string(), Expr::Parameter("tenant".to_string())),
                    ],
                )],
                edges: vec![],
            }],
            child: Some(Box::new(Operator::Unwind {
                list_expr: Expr::Parameter("resources".to_string()),
                alias: Identifier::new("resource"),
                child: Box::new(Operator::SingleRow),
            })),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();

        assert_eq!(result.stats.nodes_created, 2);
        assert_eq!(result.rows.len(), 2);
        for row in &result.rows {
            assert!(row.contains_key("resource"));
            assert!(row.contains_key("n"));
            assert!(!row.contains_key("$resources"));
            assert!(!row.contains_key("$tenant"));
        }

        let node_ids = store.find_nodes_by_label("g", "Person").unwrap();
        let mut names = Vec::new();
        for node_id in node_ids {
            let node = store.get_node("g", node_id).unwrap().unwrap();
            names.push(node.properties.get("name").cloned().unwrap());
            assert_eq!(
                node.properties.get("tenant"),
                Some(&Value::Str("acme".into()))
            );
        }
        names.sort_by(|left, right| format!("{left:?}").cmp(&format!("{right:?}")));
        assert_eq!(
            names,
            vec![Value::Str("Alice".into()), Value::Str("Bob".into())]
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
    fn create_reuses_pending_alias_in_same_pattern() {
        use crate::ast::RelType;

        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![
                    (Some(Identifier::new("a")), vec![Label::new("P")], vec![]),
                    (Some(Identifier::new("a")), vec![Label::new("P")], vec![]),
                ],
                edges: vec![(
                    Some(Identifier::new("r")),
                    Identifier::new("a"),
                    Identifier::new("a"),
                    RelType::new("SELF"),
                    Direction::Outgoing,
                    vec![],
                )],
            }],
            child: None,
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();

        assert_eq!(
            result.stats.nodes_created, 1,
            "reusing the same alias in one CREATE pattern must not allocate two nodes"
        );
        assert_eq!(result.stats.edges_created, 1);

        let a_id = match result.rows[0].get("a") {
            Some(Value::Node(id)) => *id,
            other => panic!("expected alias a to bind a node, got {other:?}"),
        };
        let edge_id = match result.rows[0].get("r") {
            Some(Value::Edge(id)) => *id,
            other => panic!("expected alias r to bind an edge, got {other:?}"),
        };
        let edge = store.get_edge("g", edge_id).unwrap().unwrap();
        assert_eq!(edge.src, a_id);
        assert_eq!(edge.dst, a_id);
    }

    #[test]
    fn create_later_node_properties_can_reference_earlier_created_alias() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![
                    (
                        Some(Identifier::new("a")),
                        vec![Label::new("Person")],
                        vec![("name".to_string(), Expr::Str("Alice".into()))],
                    ),
                    (
                        Some(Identifier::new("b")),
                        vec![Label::new("Person")],
                        vec![(
                            "friend".to_string(),
                            Expr::Property(
                                Box::new(Expr::Variable(Identifier::new("a"))),
                                "name".into(),
                            ),
                        )],
                    ),
                ],
                edges: vec![],
            }],
            child: None,
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();

        assert_eq!(result.stats.nodes_created, 2);
        let b_id = match result.rows[0].get("b") {
            Some(Value::Node(id)) => *id,
            other => panic!("expected alias b to bind a node, got {other:?}"),
        };
        let b_node = store.get_node("g", b_id).unwrap().unwrap();
        assert_eq!(
            b_node.properties.get("friend"),
            Some(&Value::Str("Alice".into())),
            "later CREATE node properties must see aliases created earlier in the same pattern"
        );
    }

    #[test]
    fn create_later_edge_properties_can_reference_earlier_created_alias() {
        use crate::ast::RelType;

        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![
                    (Some(Identifier::new("a")), vec![Label::new("P")], vec![]),
                    (Some(Identifier::new("b")), vec![Label::new("P")], vec![]),
                    (Some(Identifier::new("c")), vec![Label::new("P")], vec![]),
                ],
                edges: vec![
                    (
                        Some(Identifier::new("r1")),
                        Identifier::new("a"),
                        Identifier::new("b"),
                        RelType::new("KNOWS"),
                        Direction::Outgoing,
                        vec![("weight".to_string(), Expr::Integer(7))],
                    ),
                    (
                        Some(Identifier::new("r2")),
                        Identifier::new("b"),
                        Identifier::new("c"),
                        RelType::new("FOLLOWS"),
                        Direction::Outgoing,
                        vec![(
                            "copied".to_string(),
                            Expr::Property(
                                Box::new(Expr::Variable(Identifier::new("r1"))),
                                "weight".into(),
                            ),
                        )],
                    ),
                ],
            }],
            child: None,
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();

        assert_eq!(result.stats.edges_created, 2);
        let r2_id = match result.rows[0].get("r2") {
            Some(Value::Edge(id)) => *id,
            other => panic!("expected alias r2 to bind an edge, got {other:?}"),
        };
        let r2 = store.get_edge("g", r2_id).unwrap().unwrap();
        assert_eq!(
            r2.properties.get("copied"),
            Some(&Value::Int(7)),
            "later CREATE edge properties must see aliases created earlier in the same pattern"
        );
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
