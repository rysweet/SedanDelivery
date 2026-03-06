//! Mutation operators: CREATE, DELETE, SET, REMOVE, MERGE.

use std::collections::{HashMap, HashSet};

use crate::ast::{Direction, Expr};
use crate::error::{ExecutionError, GqlError, StorageError};
use crate::executor::budget::ExecutionBudget;
use crate::executor::filter::Row as FilterRow;
use crate::planner::{Operator, RemoveItem, SetItem};
use crate::storage::{EdgeId, NodeId};
use crate::types::Value;

use super::helpers::{eval_prop_map, extract_node_id, inject_parameters, undefined_var};
use super::{MutationStats, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_create(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Create { patterns, child } = op else {
            unreachable!()
        };
        // Execute child first (for MATCH ... CREATE patterns).
        let input = if let Some(ch) = child {
            self.exec_operator(graph, ch, budget, outer_rows, stats)?
        } else {
            // No child means standalone CREATE: one row with parameters.
            let mut row = Row::new();
            inject_parameters(&mut row, &self.parameters);
            vec![row]
        };

        let mut out = Vec::new();
        for mut row in input {
            budget.check_or_err()?;
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
                    let ctx = self.build_filter_ctx(graph, &row)?;
                    let props_raw = eval_prop_map(graph, self, prop_exprs, &row, &ctx)?;
                    // In Cypher, setting a property to null on CREATE
                    // does not store it — filter out null values.
                    let props: HashMap<String, Value> = props_raw
                        .into_iter()
                        .filter(|(_, v)| !v.is_null())
                        .collect();
                    let label_strings: Vec<String> =
                        labels.iter().map(|l| l.as_str().to_string()).collect();
                    let non_null_prop_keys: Vec<String> = props.keys().cloned().collect();
                    let node_id = self
                        .store
                        .create_node(graph, label_strings.clone(), props)?;
                    stats.nodes_created += 1;
                    stats.nodes_created_ids.insert(node_id.0);
                    stats.labels_seen.extend(label_strings);
                    for k in non_null_prop_keys {
                        stats.props_seen.insert((false, node_id.0, k));
                    }
                    if let Some(a) = alias {
                        alias_to_id.insert(a.0.clone(), node_id);
                        row.insert(a.0.clone(), Value::Node(node_id));
                    }
                }

                // Create edges using the alias map for src/dst lookup.
                for (alias, src_alias, dst_alias, rel_type, direction, prop_exprs) in &pattern.edges
                {
                    let ctx = self.build_filter_ctx(graph, &row)?;
                    // Filter out null-valued properties (same as for nodes).
                    let props: HashMap<String, Value> =
                        eval_prop_map(graph, self, prop_exprs, &row, &ctx)?
                            .into_iter()
                            .filter(|(_, v)| !v.is_null())
                            .collect();

                    let src_id = alias_to_id
                        .get(&src_alias.0)
                        .copied()
                        .or_else(|| {
                            row.get(&src_alias.0).and_then(|v| {
                                if let Value::Node(id) = v {
                                    Some(*id)
                                } else {
                                    None
                                }
                            })
                        })
                        .ok_or_else(|| undefined_var(&src_alias.0))?;

                    let dst_id = alias_to_id
                        .get(&dst_alias.0)
                        .copied()
                        .or_else(|| {
                            row.get(&dst_alias.0).and_then(|v| {
                                if let Value::Node(id) = v {
                                    Some(*id)
                                } else {
                                    None
                                }
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
                        graph,
                        actual_src,
                        actual_dst,
                        rel_type.as_str().to_string(),
                        props,
                    )?;
                    stats.edges_created += 1;
                    stats.edges_created_ids.insert(edge_id.0);
                    for k in non_null_edge_keys {
                        stats.props_seen.insert((true, edge_id.0, k));
                    }
                    if let Some(a) = alias {
                        row.insert(a.0.clone(), Value::Edge(edge_id));
                    }
                }
            }
            budget.record_row();
            out.push(row);
        }
        Ok(out)
    }

    pub(crate) fn exec_delete(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Delete {
            exprs,
            detach,
            child,
        } = op
        else {
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        let mut out = Vec::new();

        for row in input {
            budget.check_or_err()?;
            let ctx = self.build_filter_ctx(graph, &row)?;

            // Two-pass delete: collect all edges and nodes first so that
            // paths sharing nodes don't fail with "node has relationships"
            // when one path's edges are deleted but another path's edges
            // are not yet deleted.
            let mut edges_to_delete: Vec<EdgeId> = Vec::new();
            let mut nodes_to_delete: Vec<NodeId> = Vec::new();

            for expr in exprs {
                let val = self.eval_with_storage(graph, expr, &row, &ctx)?;
                match val {
                    Value::Node(node_id) => {
                        nodes_to_delete.push(node_id);
                    }
                    Value::Edge(edge_id) => {
                        // Cache rel_type before deleting so type(r) still works.
                        if let Ok(Some(data)) = self.store.get_edge(graph, edge_id) {
                            self.deleted_edge_types
                                .borrow_mut()
                                .insert(edge_id.0, data.rel_type);
                        }
                        edges_to_delete.push(edge_id);
                    }
                    Value::Null => {
                        // Deleting NULL is a no-op in Cypher
                    }
                    Value::Path { nodes, edges } => {
                        // Cache rel_type before deleting so type(r) still works.
                        for &eid in &edges {
                            if let Ok(Some(data)) = self.store.get_edge(graph, eid) {
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
                match self.store.delete_edge(graph, edge_id) {
                    Ok(()) => {
                        stats.edges_deleted += 1;
                    }
                    Err(StorageError::EdgeNotFound { .. }) => {} // already deleted
                    Err(e) => return Err(GqlError::Storage(e)),
                }
            }

            // Then delete all collected nodes.
            for node_id in nodes_to_delete {
                if *detach {
                    // Count and delete remaining edges before deleting node.
                    let out_ids = self.store.out_edge_ids(graph, node_id)?;
                    let in_ids = self.store.in_edge_ids(graph, node_id)?;
                    let mut edge_set: std::collections::HashSet<u64> =
                        out_ids.iter().map(|e| e.0).collect();
                    edge_set.extend(in_ids.iter().map(|e| e.0));
                    // Net out edges that were created in this same query.
                    let mut net_deleted_edges: u64 = 0;
                    for eid in &edge_set {
                        if stats.edges_created_ids.remove(eid) {
                            stats.edges_created = stats.edges_created.saturating_sub(1);
                        } else {
                            net_deleted_edges += 1;
                        }
                    }
                    match self.store.delete_node(graph, node_id, true) {
                        Ok(()) => {
                            // Net out: if node was created in this query, cancel the create.
                            if stats.nodes_created_ids.remove(&node_id.0) {
                                stats.nodes_created = stats.nodes_created.saturating_sub(1);
                            } else {
                                stats.nodes_deleted += 1;
                            }
                            stats.edges_deleted += net_deleted_edges;
                        }
                        Err(StorageError::NodeNotFound { .. }) => {}
                        Err(e) => return Err(GqlError::Storage(e)),
                    }
                } else {
                    match self.store.delete_node(graph, node_id, false) {
                        Ok(()) => {
                            // Net out: if node was created in this query, cancel the create.
                            if stats.nodes_created_ids.remove(&node_id.0) {
                                stats.nodes_created = stats.nodes_created.saturating_sub(1);
                            } else {
                                stats.nodes_deleted += 1;
                            }
                        }
                        Err(StorageError::NodeNotFound { .. }) => {} // already deleted
                        Err(e) => return Err(GqlError::Storage(e)),
                    }
                }
            }
            budget.record_row();
            out.push(row);
        }
        Ok(out)
    }

    pub(crate) fn exec_set(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Set { items, child } = op else {
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            budget.check_or_err()?;
            let current_row = row;
            for item in items {
                self.apply_set_item(graph, &current_row, item, stats)?;
            }
            budget.record_row();
            out.push(current_row);
        }
        Ok(out)
    }

    pub(crate) fn exec_remove(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Remove { items, child } = op else {
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            budget.check_or_err()?;
            for item in items {
                match item {
                    RemoveItem::Property { alias, key } => {
                        let val = row
                            .get(alias.0.as_str())
                            .ok_or_else(|| undefined_var(&alias.0))?;
                        match val {
                            Value::Null => {
                                // REMOVE on null (from OPTIONAL MATCH) is a no-op
                            }
                            Value::Node(node_id) => {
                                self.store.remove_node_property(graph, *node_id, key)?;
                            }
                            Value::Edge(edge_id) => {
                                self.store.remove_edge_property(graph, *edge_id, key)?;
                            }
                            other => {
                                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                    expected: "Node or Relationship".to_string(),
                                    actual: other.type_name().to_string(),
                                }));
                            }
                        }
                        // REMOVE property does not count as properties_set.
                    }
                    RemoveItem::Labels { alias, labels } => {
                        let val = row
                            .get(alias.0.as_str())
                            .ok_or_else(|| undefined_var(&alias.0))?;
                        // REMOVE labels on null (from OPTIONAL MATCH) is a no-op
                        if val.is_null() {
                            continue;
                        }
                        let node_id = extract_node_id(val, &alias.0)?;
                        let label_strings: Vec<String> =
                            labels.iter().map(|l| l.as_str().to_string()).collect();
                        let removed_count = label_strings.len() as u64;
                        self.store
                            .remove_node_labels(graph, node_id, &label_strings)?;
                        stats.labels_removed += removed_count;
                    }
                }
            }
            budget.record_row();
            out.push(row);
        }
        Ok(out)
    }

    /// Evaluate MERGE property expressions, expanding parameter maps and
    /// rejecting null values.  Shared by `exec_merge` and `exec_merge_rel`.
    pub(crate) fn eval_merge_properties(
        &self,
        graph: &str,
        properties: &[(String, Expr)],
        row: &Row,
        ctx: &FilterRow<'_>,
    ) -> Result<Vec<(String, Value)>, GqlError> {
        let mut prop_vals: Vec<(String, Value)> = Vec::new();
        for (k, expr) in properties {
            if k.is_empty() {
                // Parameter map expansion sentinel.
                let val = self.eval_with_storage(graph, expr, row, ctx)?;
                match val {
                    Value::Map(entries) => {
                        for (mk, mv) in entries {
                            if mv.is_null() {
                                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                    expected: "non-null property value in MERGE".to_string(),
                                    actual: "Null".to_string(),
                                }));
                            }
                            prop_vals.push((mk.to_string(), mv));
                        }
                    }
                    Value::Null => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "non-null Map for MERGE parameter properties".to_string(),
                            actual: "Null".to_string(),
                        }));
                    }
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "Map for MERGE parameter properties".to_string(),
                            actual: other.type_name().to_string(),
                        }));
                    }
                }
            } else {
                let val = self.eval_with_storage(graph, expr, row, ctx)?;
                // MERGE with null property is a SemanticError (MergeReadOwnWrites)
                if val.is_null() {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "non-null property value in MERGE".to_string(),
                        actual: "Null".to_string(),
                    }));
                }
                prop_vals.push((k.clone(), val));
            }
        }
        Ok(prop_vals)
    }

    pub(crate) fn exec_merge(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Merge {
            node_alias,
            labels,
            properties,
            on_create,
            on_match,
            child,
        } = op
        else {
            unreachable!()
        };
        let input = if let Some(ch) = child {
            self.exec_operator(graph, ch, budget, outer_rows, stats)?
        } else {
            // Standalone MERGE (no preceding clause): one row with parameters.
            let mut row = Row::new();
            inject_parameters(&mut row, &self.parameters);
            vec![row]
        };

        let mut out = Vec::new();
        for mut row in input {
            budget.check_or_err()?;

            // Evaluate inline property values from the MERGE pattern.
            let prop_vals = if !properties.is_empty() {
                let ctx = self.build_filter_ctx(graph, &row)?;
                self.eval_merge_properties(graph, properties, &row, &ctx)?
            } else {
                Vec::new()
            };

            // Try to find existing nodes matching ALL labels and properties.
            // Fast path: when there is at least one label and one property, use
            // find_nodes_by_label_and_property for the first (label, property)
            // pair. This is O(1) when a property index exists (vs O(N) label scan).
            let candidates: HashSet<u64> = if labels.is_empty() {
                self.store
                    .all_node_ids(graph)?
                    .into_iter()
                    .map(|n| n.0)
                    .collect()
            } else if !prop_vals.is_empty() {
                // Use indexed lookup on the first property for the first label,
                // then intersect with remaining labels and verify remaining props.
                let first_label = labels[0].as_str();
                let (first_prop_key, first_prop_val) = &prop_vals[0];
                let indexed_ids = self.store.find_nodes_by_label_and_property(
                    graph,
                    first_label,
                    first_prop_key,
                    crate::storage::RangeQuery::Eq(first_prop_val),
                )?;
                let mut cands: HashSet<u64> = indexed_ids.into_iter().map(|n| n.0).collect();
                // Intersect with remaining labels.
                for label in &labels[1..] {
                    let ids = self.store.find_nodes_by_label(graph, label.as_str())?;
                    let id_set: HashSet<u64> = ids.iter().map(|n| n.0).collect();
                    cands = cands.intersection(&id_set).copied().collect();
                }
                cands
            } else {
                let mut cands: Option<HashSet<u64>> = None;
                for label in labels {
                    let ids = self.store.find_nodes_by_label(graph, label.as_str())?;
                    let id_set: HashSet<u64> = ids.iter().map(|n| n.0).collect();
                    cands = Some(match cands {
                        None => id_set,
                        Some(existing) => existing.intersection(&id_set).copied().collect(),
                    });
                }
                cands.unwrap_or_default()
            };

            // Check candidates for matching nodes. MERGE returns one row per
            // match; if no matches are found, it creates a new node instead.
            // When the indexed fast-path was used, candidates are already filtered
            // by the first property — only verify remaining properties.
            let skip_first_prop = !labels.is_empty() && !prop_vals.is_empty();
            let check_props = if skip_first_prop {
                &prop_vals[1..]
            } else {
                &prop_vals[..]
            };
            let mut matched_ids: Vec<NodeId> = Vec::new();
            for cand_id in &candidates {
                budget.check_or_err()?;
                let node_id = NodeId(*cand_id);
                if prop_vals.is_empty() {
                    matched_ids.push(node_id);
                } else if check_props.is_empty() {
                    // All properties already matched by index lookup.
                    matched_ids.push(node_id);
                } else if let Ok(Some(node_data)) = self.store.get_node(graph, node_id) {
                    let all_match = check_props.iter().all(|(key, expected)| {
                        node_data
                            .properties
                            .get(key.as_str())
                            .map(|v| v == expected)
                            .unwrap_or(false)
                    });
                    if all_match {
                        matched_ids.push(node_id);
                    }
                }
            }

            if !matched_ids.is_empty() {
                // ON MATCH: one output row per matched node
                for node_id in &matched_ids {
                    budget.check_or_err()?;
                    let mut new_row = row.clone();
                    if let Some(alias) = node_alias {
                        new_row.insert(alias.0.clone(), Value::Node(*node_id));
                    }
                    for item in on_match {
                        self.apply_set_item(graph, &new_row, item, stats)?;
                    }
                    budget.record_row();
                    out.push(new_row);
                }
            } else {
                // Not found: CREATE the node with inline properties
                let label_strings: Vec<String> =
                    labels.iter().map(|l| l.as_str().to_string()).collect();
                let create_props: HashMap<String, Value> = prop_vals.iter().cloned().collect();
                let non_null_prop_keys: Vec<String> = create_props
                    .iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, _)| k.clone())
                    .collect();
                let node_id = self
                    .store
                    .create_node(graph, label_strings.clone(), create_props)?;
                stats.nodes_created += 1;
                stats.nodes_created_ids.insert(node_id.0);
                stats.labels_seen.extend(label_strings);
                for k in non_null_prop_keys {
                    stats.props_seen.insert((false, node_id.0, k));
                }
                if let Some(alias) = node_alias {
                    row.insert(alias.0.clone(), Value::Node(node_id));
                }
                for item in on_create {
                    self.apply_set_item(graph, &row, item, stats)?;
                }
                budget.record_row();
                out.push(row);
            }
        }
        Ok(out)
    }

    pub(crate) fn exec_merge_rel(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::MergeRelationship {
            src_alias,
            dst_alias,
            rel_alias,
            rel_type,
            direction,
            properties,
            on_create,
            on_match,
            child,
        } = op
        else {
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        let type_str = rel_type.as_str();
        let mut out = Vec::new();

        for mut row in input {
            budget.check_or_err()?;
            let src_val = row
                .get(src_alias.0.as_str())
                .ok_or_else(|| undefined_var(&src_alias.0))?;
            let src_id = extract_node_id(src_val, &src_alias.0)?;

            let dst_val = row
                .get(dst_alias.0.as_str())
                .ok_or_else(|| undefined_var(&dst_alias.0))?;
            let dst_id = extract_node_id(dst_val, &dst_alias.0)?;

            // Determine actual src/dst based on direction.
            let (real_src, real_dst) = match direction {
                Direction::Incoming => (dst_id, src_id),
                _ => (src_id, dst_id),
            };

            // Find existing edges of this type between src and dst.
            // For undirected patterns, check BOTH directions (a->b and b->a).
            let existing_edges = self.store.out_edges_by_type(graph, real_src, &[type_str])?;
            let mut matching: Vec<_> = existing_edges
                .into_iter()
                .filter(|adj| adj.neighbor_id == real_dst)
                .collect();
            if matches!(direction, Direction::Undirected) && matching.is_empty() {
                // Also check the reverse direction (real_dst -> real_src).
                let reverse_edges = self.store.out_edges_by_type(graph, real_dst, &[type_str])?;
                matching = reverse_edges
                    .into_iter()
                    .filter(|adj| adj.neighbor_id == real_src)
                    .collect();
            }

            // Filter by property constraints if any.
            let ctx = self.build_filter_ctx(graph, &row)?;
            let prop_vals = self.eval_merge_properties(graph, properties, &row, &ctx)?;

            let matched_edges: Vec<EdgeId> = if prop_vals.is_empty() {
                matching.iter().map(|adj| adj.edge_id).collect::<Vec<_>>()
            } else {
                // Find ALL edges matching property constraints.
                let mut found_edges = Vec::new();
                for adj in &matching {
                    if let Some(edge_data) = self.store.get_edge(graph, adj.edge_id)? {
                        let all_match = prop_vals.iter().all(|(k, v)| {
                            edge_data
                                .properties
                                .get(k.as_str())
                                .map(|ev| ev == v)
                                .unwrap_or(false)
                        });
                        if all_match {
                            found_edges.push(adj.edge_id);
                        }
                    }
                }
                found_edges
            };

            if !matched_edges.is_empty() {
                // MATCHED: one output row per matched edge.
                for edge_id in matched_edges {
                    budget.check_or_err()?;
                    let mut new_row = row.clone();
                    if let Some(ra) = rel_alias {
                        new_row.insert(ra.0.clone(), Value::Edge(edge_id));
                    }
                    for item in on_match {
                        self.apply_set_item(graph, &new_row, item, stats)?;
                    }
                    budget.record_row();
                    out.push(new_row);
                }
            } else {
                // NOT FOUND: create the edge with properties.
                let prop_map: HashMap<String, Value> = prop_vals.into_iter().collect();
                let non_null_edge_keys: Vec<String> = prop_map
                    .iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, _)| k.clone())
                    .collect();
                let edge_id = self.store.create_edge(
                    graph,
                    real_src,
                    real_dst,
                    type_str.to_string(),
                    prop_map,
                )?;
                stats.edges_created += 1;
                for k in non_null_edge_keys {
                    stats.props_seen.insert((true, edge_id.0, k));
                }
                if let Some(ra) = rel_alias {
                    row.insert(ra.0.clone(), Value::Edge(edge_id));
                }
                for item in on_create {
                    self.apply_set_item(graph, &row, item, stats)?;
                }
                budget.record_row();
                out.push(row);
            }
        }
        Ok(out)
    }

    /// Apply a single `SetItem` (SET n.prop = val, SET n = {map}, SET n += {map}, SET n:Label).
    /// Handles both Node and Edge targets for property operations.
    pub(crate) fn apply_set_item(
        &self,
        graph: &str,
        row: &Row,
        item: &SetItem,
        stats: &mut MutationStats,
    ) -> Result<(), GqlError> {
        match item {
            SetItem::Property { alias, key, value } => {
                let val = row
                    .get(alias.0.as_str())
                    .ok_or_else(|| undefined_var(&alias.0))?;
                // SET on a null variable (from OPTIONAL MATCH) is silently ignored.
                if val.is_null() {
                    return Ok(());
                }
                let ctx = self.build_filter_ctx(graph, row)?;
                let prop_val = self.eval_with_storage(graph, value, row, &ctx)?;
                // SET n.prop = null means REMOVE the property (Cypher semantics)
                if prop_val.is_null() {
                    match val {
                        Value::Node(node_id) => {
                            self.store.remove_node_property(graph, *node_id, key)?;
                        }
                        Value::Edge(edge_id) => {
                            self.store.remove_edge_property(graph, *edge_id, key)?;
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // Validate that the property value is a valid Cypher property type.
                // Maps and lists-of-maps are not allowed as property values.
                fn is_invalid_prop_value(v: &Value) -> bool {
                    match v {
                        Value::Map(_) => true,
                        Value::List(items) => items.iter().any(is_invalid_prop_value),
                        _ => false,
                    }
                }
                if is_invalid_prop_value(&prop_val) {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "primitive property value (not Map or List of Maps)".to_string(),
                        actual: prop_val.type_name().to_string(),
                    }));
                }
                let mut props = HashMap::new();
                props.insert(key.clone(), prop_val);
                match val {
                    Value::Node(node_id) => {
                        self.store.set_node_properties(graph, *node_id, props)?;
                        stats.props_seen.insert((false, node_id.0, key.clone()));
                    }
                    Value::Edge(edge_id) => {
                        self.store.set_edge_properties(graph, *edge_id, props)?;
                        stats.props_seen.insert((true, edge_id.0, key.clone()));
                    }
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: format!("Node or Relationship for variable '{}'", alias.0),
                            actual: other.type_name().to_string(),
                        }));
                    }
                }
            }
            SetItem::Map { alias, map, merge } => {
                let val = row
                    .get(alias.0.as_str())
                    .ok_or_else(|| undefined_var(&alias.0))?;
                // SET on a null variable (from OPTIONAL MATCH) is silently ignored.
                if val.is_null() {
                    return Ok(());
                }
                let ctx = self.build_filter_ctx(graph, row)?;
                let map_val = self.eval_with_storage(graph, map, row, &ctx)?;
                let props = match map_val {
                    Value::Map(m) => m
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect::<HashMap<String, Value>>(),
                    Value::Null => HashMap::new(),
                    Value::Node(src_node_id) => {
                        // SET a = b where b is a Node: copy all properties from b
                        match self.store.get_node(graph, src_node_id)? {
                            Some(node_data) => node_data.properties,
                            None => HashMap::new(),
                        }
                    }
                    Value::Edge(src_edge_id) => {
                        // SET r = r2 where r2 is a Relationship: copy all properties from r2
                        match self.store.get_edge(graph, src_edge_id)? {
                            Some(edge_data) => edge_data.properties,
                            None => HashMap::new(),
                        }
                    }
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "Map for SET assignment".to_string(),
                            actual: other.type_name().to_string(),
                        }));
                    }
                };
                let non_null_keys: Vec<String> = props
                    .iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, _)| k.clone())
                    .collect();
                let (is_edge, raw_id) = match val {
                    Value::Node(node_id) => {
                        let id = *node_id;
                        if *merge {
                            self.store.set_node_properties(graph, id, props)?;
                        } else {
                            self.store.replace_node_properties(graph, id, props)?;
                        }
                        (false, id.0)
                    }
                    Value::Edge(edge_id) => {
                        let id = *edge_id;
                        if *merge {
                            self.store.set_edge_properties(graph, id, props)?;
                        } else {
                            self.store.replace_edge_properties(graph, id, props)?;
                        }
                        (true, id.0)
                    }
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: format!("Node or Relationship for variable '{}'", alias.0),
                            actual: other.type_name().to_string(),
                        }));
                    }
                };
                for k in non_null_keys {
                    stats.props_seen.insert((is_edge, raw_id, k));
                }
            }
            SetItem::Labels { alias, labels } => {
                let val = row
                    .get(alias.0.as_str())
                    .ok_or_else(|| undefined_var(&alias.0))?;
                // SET labels on null (from OPTIONAL MATCH) is a no-op
                if val.is_null() {
                    return Ok(());
                }
                let node_id = extract_node_id(val, &alias.0)?;
                let label_strings: Vec<String> =
                    labels.iter().map(|l| l.as_str().to_string()).collect();
                self.store.add_node_labels(graph, node_id, &label_strings)?;
                stats.labels_seen.extend(label_strings);
            }
        }
        Ok(())
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
    fn set_property() {
        let store = make_store();
        let nid = store
            .create_node("g", vec!["P".to_string()], HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Set {
            items: vec![SetItem::Property {
                alias: Identifier::new("n"),
                key: "x".to_string(),
                value: Expr::Integer(1),
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.properties_set, 1);
        assert_eq!(
            store
                .get_node("g", nid)
                .unwrap()
                .unwrap()
                .properties
                .get("x"),
            Some(&Value::Int(1))
        );
    }

    #[test]
    fn remove_property() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("x".to_string(), Value::Int(1));
        let nid = store
            .create_node("g", vec!["P".to_string()], props)
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Remove {
            items: vec![RemoveItem::Property {
                alias: Identifier::new("n"),
                key: "x".to_string(),
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let _ = executor.execute("g", &plan, &mut budget).unwrap();
        assert!(!store
            .get_node("g", nid)
            .unwrap()
            .unwrap()
            .properties
            .contains_key("x"));
    }

    #[test]
    fn merge_creates_when_not_found() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("P")],
            properties: vec![],
            on_create: vec![],
            on_match: vec![],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 1);
    }

    #[test]
    fn merge_matches_existing() {
        let store = make_store();
        store
            .create_node("g", vec!["P".to_string()], HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("P")],
            properties: vec![],
            on_create: vec![],
            on_match: vec![],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 0);
    }

    /// Verify that storage errors during SET n.prop = null (property removal)
    /// are propagated instead of silently discarded.
    #[test]
    fn set_null_propagates_removal_error() {
        let store = make_store();
        // Create and then delete a node so the NodeId becomes stale.
        let nid = store
            .create_node("g", vec!["P".to_string()], HashMap::new())
            .unwrap();
        store.delete_node("g", nid, true).unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("n".to_string(), Value::Node(nid));
        let mut stats = MutationStats::default();

        // SET n.prop = null should attempt remove_node_property on a
        // non-existent node and propagate the StorageError.
        let item = SetItem::Property {
            alias: Identifier::new("n"),
            key: "prop".to_string(),
            value: Expr::Null,
        };
        let result = executor.apply_set_item("g", &row, &item, &mut stats);
        assert!(
            result.is_err(),
            "expected error when removing property from deleted node"
        );
    }

    /// Verify that MERGE returns all matching nodes (one row per match)
    /// and does not create when matches exist.
    #[test]
    fn merge_returns_all_matches_no_create() {
        let store = make_store();
        // Create 5 nodes with the same label.
        for _ in 0..5 {
            store
                .create_node("g", vec!["P".to_string()], HashMap::new())
                .unwrap();
        }
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("P")],
            properties: vec![],
            on_create: vec![],
            on_match: vec![],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // All 5 existing nodes should match; no new nodes created.
        assert_eq!(result.stats.nodes_created, 0);
        assert_eq!(
            result.rows.len(),
            5,
            "MERGE should produce one row per matching node"
        );
    }
}
