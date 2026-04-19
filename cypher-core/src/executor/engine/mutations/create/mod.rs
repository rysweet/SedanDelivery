//! CREATE operator implementation.
//!
//! Split into focused submodules:
//! - [`alias_refs`] expression-walker helpers used to decide when batched
//!   creates must flush so later pattern items can read earlier aliases
//! - [`pending`]    buffered node/edge create requests
//!
//! The single `impl QueryExecutor::exec_create` entry point lives here.

use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

use crate::ast::Direction;
use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::storage::NodeId;
use crate::types::Value;

use crate::executor::engine::helpers::{eval_prop_map, inject_parameters, undefined_var};
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

mod alias_refs;
mod pending;

#[cfg(test)]
mod tests;

use alias_refs::prop_exprs_reference_any_alias;
use pending::{PendingCreateEdge, PendingCreateNode};

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
                        created_node_ids.into_iter().zip(metadata)
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
                        created_edge_ids.into_iter().zip(metadata)
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
