//! MERGE and MERGE RELATIONSHIP operator implementations.

use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

use crate::ast::{Direction, Expr};
use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::Row as FilterRow;
use crate::planner::Operator;
use crate::storage::NodeId;
use crate::types::Value;

use crate::executor::engine::helpers::{extract_node_id, inject_parameters, undefined_var};
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PendingMergeEdgeKey {
    src: NodeId,
    dst: NodeId,
    rel_type: String,
    props_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PendingMergeNodeKey {
    labels: Vec<String>,
    props_key: String,
}

enum PendingMergeNodeSlot {
    Ready(Vec<Row>),
    Create { row: Row, create_index: usize },
    MatchedPending { row: Row, create_index: usize },
}

enum PendingMergeEdgeSlot {
    Ready(Vec<Row>),
    Pending { row: Row, create_index: usize },
}

struct MergeRelBatchSpec<'a> {
    src_alias: &'a crate::ast::Identifier,
    dst_alias: &'a crate::ast::Identifier,
    rel_alias: Option<&'a crate::ast::Identifier>,
    rel_type: &'a crate::ast::RelType,
    direction: Direction,
    properties: &'a [(String, Expr)],
}

impl<'a> QueryExecutor<'a> {
    fn merge_node_props_key(properties: &IndexMap<String, Value>) -> String {
        let canonical_props = properties
            .iter()
            .map(|(k, v)| (k.clone().into_boxed_str(), v.clone()))
            .collect();
        Value::Map(canonical_props).canonical_key()
    }

    fn merge_relationship_props_key(properties: &IndexMap<String, Value>) -> String {
        let canonical_props = properties
            .iter()
            .map(|(k, v)| (k.clone().into_boxed_str(), v.clone()))
            .collect();
        Value::Map(canonical_props).canonical_key()
    }

    fn merge_relationship_props_map(prop_vals: Vec<(String, Value)>) -> IndexMap<String, Value> {
        prop_vals.into_iter().collect()
    }

    fn exec_merge_rel_batch_creates(
        &self,
        cx: &mut ExecContext<'_>,
        input: Vec<Row>,
        spec: MergeRelBatchSpec<'_>,
    ) -> Result<Vec<Row>, GqlError> {
        let type_str = spec.rel_type.as_str();
        let type_owned = type_str.to_string();
        let mut pending_creates: Vec<(NodeId, NodeId, String, IndexMap<String, Value>)> =
            Vec::new();
        let mut pending_non_null_keys: Vec<Vec<String>> = Vec::new();
        let mut pending_by_key: HashMap<PendingMergeEdgeKey, usize> = HashMap::new();
        let mut slots = Vec::with_capacity(input.len());

        for mut row in input {
            cx.budget.check_or_err()?;
            let src_val = row
                .get(spec.src_alias.0.as_str())
                .ok_or_else(|| undefined_var(&spec.src_alias.0))?;
            let src_id = extract_node_id(src_val, &spec.src_alias.0)?;

            let dst_val = row
                .get(spec.dst_alias.0.as_str())
                .ok_or_else(|| undefined_var(&spec.dst_alias.0))?;
            let dst_id = extract_node_id(dst_val, &spec.dst_alias.0)?;

            let (real_src, real_dst) = match spec.direction {
                Direction::Incoming => (dst_id, src_id),
                _ => (src_id, dst_id),
            };

            let ctx = self.build_filter_ctx(cx.graph, &row)?;
            let prop_vals = self.eval_merge_properties(cx.graph, spec.properties, &row, &ctx)?;
            let prop_map = Self::merge_relationship_props_map(prop_vals);
            let dedup_key = PendingMergeEdgeKey {
                src: if matches!(spec.direction, Direction::Undirected) && real_src.0 > real_dst.0 {
                    real_dst
                } else {
                    real_src
                },
                dst: if matches!(spec.direction, Direction::Undirected) && real_src.0 > real_dst.0 {
                    real_src
                } else {
                    real_dst
                },
                rel_type: type_owned.clone(),
                props_key: Self::merge_relationship_props_key(&prop_map),
            };

            if let Some(&create_index) = pending_by_key.get(&dedup_key) {
                slots.push(PendingMergeEdgeSlot::Pending { row, create_index });
                continue;
            }

            // One store round-trip per unique (src, dst, type, props) key;
            // the dedup HashMap above prevents redundant calls for identical keys.
            let existing_edges = self
                .store
                .out_edges_by_type(cx.graph, real_src, &[type_str])?;
            let mut matching: Vec<_> = existing_edges
                .into_iter()
                .filter(|adj| adj.neighbor_id == real_dst)
                .collect();
            if matches!(spec.direction, Direction::Undirected) && matching.is_empty() {
                let reverse_edges =
                    self.store
                        .out_edges_by_type(cx.graph, real_dst, &[type_str])?;
                matching = reverse_edges
                    .into_iter()
                    .filter(|adj| adj.neighbor_id == real_src)
                    .collect();
            }

            let matched_edges = if prop_map.is_empty() {
                matching.iter().map(|adj| adj.edge_id).collect::<Vec<_>>()
            } else {
                let mut found_edges = Vec::new();
                for adj in &matching {
                    if let Some(edge_data) = self.store.get_edge(cx.graph, adj.edge_id)? {
                        let all_match = prop_map.iter().all(|(k, v)| {
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

            if matched_edges.is_empty() {
                let create_index = pending_creates.len();
                pending_non_null_keys.push(
                    prop_map
                        .iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(k, _)| k.clone())
                        .collect(),
                );
                pending_creates.push((real_src, real_dst, type_str.to_string(), prop_map));
                pending_by_key.insert(dedup_key, create_index);
                slots.push(PendingMergeEdgeSlot::Pending { row, create_index });
                continue;
            }

            let mut ready_rows = Vec::with_capacity(matched_edges.len());
            for edge_id in matched_edges {
                cx.budget.check_or_err()?;
                if let Some(alias) = spec.rel_alias {
                    row.insert(alias.0.clone(), Value::Edge(edge_id));
                }
                cx.budget.record_row();
                ready_rows.push(row.clone());
            }
            slots.push(PendingMergeEdgeSlot::Ready(ready_rows));
        }

        let mut created_edge_ids = Vec::new();
        if !pending_creates.is_empty() {
            created_edge_ids = self.store.batch_create_edges(cx.graph, pending_creates)?;
            for (create_index, edge_id) in created_edge_ids.iter().enumerate() {
                cx.stats.edges_created += 1;
                cx.stats.edges_created_ids.insert(edge_id.0);
                for key in &pending_non_null_keys[create_index] {
                    cx.stats.props_seen.insert((true, edge_id.0, key.clone()));
                }
            }
        }

        let mut out = Vec::new();
        for slot in slots {
            match slot {
                PendingMergeEdgeSlot::Ready(rows) => out.extend(rows),
                PendingMergeEdgeSlot::Pending {
                    mut row,
                    create_index,
                } => {
                    let edge_id = created_edge_ids[create_index];
                    if let Some(alias) = spec.rel_alias {
                        row.insert(alias.0.clone(), Value::Edge(edge_id));
                    }
                    cx.budget.record_row();
                    out.push(row);
                }
            }
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
        cx: &mut ExecContext<'_>,
        op: &Operator,
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
            debug_assert!(false, "expected Merge operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Merge operator".into(),
            }));
        };
        let input = if let Some(ch) = child {
            self.exec_operator(cx, ch)?
        } else {
            // Standalone MERGE (no preceding clause): one row with parameters.
            let mut row = Row::new();
            inject_parameters(&mut row, &self.parameters);
            vec![row]
        };
        let label_strings: Vec<String> = labels.iter().map(|l| l.as_str().to_string()).collect();
        if on_create.is_empty() && on_match.is_empty() {
            let mut pending_creates: Vec<(Vec<String>, IndexMap<String, Value>)> = Vec::new();
            let mut pending_non_null_keys: Vec<Vec<String>> = Vec::new();
            let mut pending_by_key: HashMap<PendingMergeNodeKey, usize> = HashMap::new();
            let mut slots = Vec::with_capacity(input.len());

            for row in input {
                cx.budget.check_or_err()?;

                let prop_vals = if !properties.is_empty() {
                    let ctx = self.build_filter_ctx(cx.graph, &row)?;
                    self.eval_merge_properties(cx.graph, properties, &row, &ctx)?
                } else {
                    Vec::new()
                };
                let create_props: IndexMap<String, Value> = prop_vals.iter().cloned().collect();
                let pending_key = PendingMergeNodeKey {
                    labels: label_strings.clone(),
                    props_key: Self::merge_node_props_key(&create_props),
                };
                if let Some(&create_index) = pending_by_key.get(&pending_key) {
                    slots.push(PendingMergeNodeSlot::MatchedPending { row, create_index });
                    continue;
                }

                let candidates: HashSet<u64> = if labels.is_empty() {
                    self.store
                        .all_node_ids(cx.graph)?
                        .into_iter()
                        .map(|n| n.0)
                        .collect()
                } else if !prop_vals.is_empty() {
                    let first_label = labels[0].as_str();
                    let (first_prop_key, first_prop_val) = &prop_vals[0];
                    let indexed_ids = self.store.find_nodes_by_label_and_property(
                        cx.graph,
                        first_label,
                        first_prop_key,
                        crate::storage::RangeQuery::Eq(first_prop_val),
                    )?;
                    let mut cands: HashSet<u64> = indexed_ids.into_iter().map(|n| n.0).collect();
                    for label in &labels[1..] {
                        let ids = self.store.find_nodes_by_label(cx.graph, label.as_str())?;
                        let id_set: HashSet<u64> = ids.iter().map(|n| n.0).collect();
                        cands = cands.intersection(&id_set).copied().collect();
                    }
                    cands
                } else {
                    let mut cands: Option<HashSet<u64>> = None;
                    for label in labels {
                        let ids = self.store.find_nodes_by_label(cx.graph, label.as_str())?;
                        let id_set: HashSet<u64> = ids.iter().map(|n| n.0).collect();
                        cands = Some(match cands {
                            None => id_set,
                            Some(existing) => existing.intersection(&id_set).copied().collect(),
                        });
                    }
                    cands.unwrap_or_default()
                };

                let skip_first_prop = !labels.is_empty() && !prop_vals.is_empty();
                let check_props = if skip_first_prop {
                    &prop_vals[1..]
                } else {
                    &prop_vals[..]
                };
                let mut matched_ids = Vec::new();
                for cand_id in &candidates {
                    cx.budget.check_or_err()?;
                    let node_id = NodeId(*cand_id);
                    // Single-property case: already matched by indexed lookup above.
                    if check_props.is_empty() {
                        matched_ids.push(node_id);
                    } else if let Ok(Some(node_data)) = self.store.get_node(cx.graph, node_id) {
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
                    let mut ready_rows = Vec::with_capacity(matched_ids.len());
                    for node_id in matched_ids {
                        let mut new_row = row.clone();
                        if let Some(alias) = node_alias {
                            new_row.insert(alias.0.clone(), Value::Node(node_id));
                        }
                        cx.budget.record_row();
                        ready_rows.push(new_row);
                    }
                    slots.push(PendingMergeNodeSlot::Ready(ready_rows));
                } else {
                    let create_index = pending_creates.len();
                    pending_non_null_keys.push(
                        create_props
                            .iter()
                            .filter(|(_, v)| !v.is_null())
                            .map(|(k, _)| k.clone())
                            .collect(),
                    );
                    pending_creates.push((label_strings.clone(), create_props));
                    pending_by_key.insert(pending_key, create_index);
                    slots.push(PendingMergeNodeSlot::Create { row, create_index });
                }
            }

            let mut created_node_ids = Vec::new();
            if !pending_creates.is_empty() {
                created_node_ids = self.store.batch_create_nodes(cx.graph, pending_creates)?;
                for (create_index, node_id) in created_node_ids.iter().enumerate() {
                    cx.stats.nodes_created += 1;
                    cx.stats.nodes_created_ids.insert(node_id.0);
                    cx.stats.labels_seen.extend(label_strings.iter().cloned());
                    for key in &pending_non_null_keys[create_index] {
                        cx.stats.props_seen.insert((false, node_id.0, key.clone()));
                    }
                }
            }

            let mut out = Vec::with_capacity(slots.len());
            for slot in slots {
                match slot {
                    PendingMergeNodeSlot::Ready(rows) => out.extend(rows),
                    PendingMergeNodeSlot::Create {
                        mut row,
                        create_index,
                    }
                    | PendingMergeNodeSlot::MatchedPending {
                        mut row,
                        create_index,
                    } => {
                        let node_id = created_node_ids[create_index];
                        if let Some(alias) = node_alias {
                            row.insert(alias.0.clone(), Value::Node(node_id));
                        }
                        cx.budget.record_row();
                        out.push(row);
                    }
                }
            }
            self.invalidate_property_cache();
            return Ok(out);
        }

        let mut out = Vec::with_capacity(input.len());
        for mut row in input {
            cx.budget.check_or_err()?;

            // Evaluate inline property values from the MERGE pattern.
            let prop_vals = if !properties.is_empty() {
                let ctx = self.build_filter_ctx(cx.graph, &row)?;
                self.eval_merge_properties(cx.graph, properties, &row, &ctx)?
            } else {
                Vec::new()
            };

            // Try to find existing nodes matching ALL labels and properties.
            // Fast path: when there is at least one label and one property, use
            // find_nodes_by_label_and_property for the first (label, property)
            // pair. This is O(1) when a property index exists (vs O(N) label scan).
            let candidates: HashSet<u64> = if labels.is_empty() {
                self.store
                    .all_node_ids(cx.graph)?
                    .into_iter()
                    .map(|n| n.0)
                    .collect()
            } else if !prop_vals.is_empty() {
                // Use indexed lookup on the first property for the first label,
                // then intersect with remaining labels and verify remaining props.
                let first_label = labels[0].as_str();
                let (first_prop_key, first_prop_val) = &prop_vals[0];
                let indexed_ids = self.store.find_nodes_by_label_and_property(
                    cx.graph,
                    first_label,
                    first_prop_key,
                    crate::storage::RangeQuery::Eq(first_prop_val),
                )?;
                let mut cands: HashSet<u64> = indexed_ids.into_iter().map(|n| n.0).collect();
                // Intersect with remaining labels.
                for label in &labels[1..] {
                    let ids = self.store.find_nodes_by_label(cx.graph, label.as_str())?;
                    let id_set: HashSet<u64> = ids.iter().map(|n| n.0).collect();
                    cands = cands.intersection(&id_set).copied().collect();
                }
                cands
            } else {
                let mut cands: Option<HashSet<u64>> = None;
                for label in labels {
                    let ids = self.store.find_nodes_by_label(cx.graph, label.as_str())?;
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
                cx.budget.check_or_err()?;
                let node_id = NodeId(*cand_id);
                if prop_vals.is_empty() {
                    matched_ids.push(node_id);
                } else if check_props.is_empty() {
                    // All properties already matched by index lookup.
                    matched_ids.push(node_id);
                } else if let Ok(Some(node_data)) = self.store.get_node(cx.graph, node_id) {
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
                    cx.budget.check_or_err()?;
                    let mut new_row = row.clone();
                    if let Some(alias) = node_alias {
                        new_row.insert(alias.0.clone(), Value::Node(*node_id));
                    }
                    for item in on_match {
                        self.apply_set_item(cx.graph, &new_row, item, cx.stats)?;
                    }
                    cx.budget.record_row();
                    out.push(new_row);
                }
            } else {
                // Not found: CREATE the node with inline properties
                let create_props: IndexMap<String, Value> = prop_vals.iter().cloned().collect();
                let non_null_prop_keys: Vec<String> = create_props
                    .iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, _)| k.clone())
                    .collect();
                let node_id =
                    self.store
                        .create_node(cx.graph, label_strings.clone(), create_props)?;
                cx.stats.nodes_created += 1;
                cx.stats.nodes_created_ids.insert(node_id.0);
                cx.stats.labels_seen.extend(label_strings.iter().cloned());
                for k in non_null_prop_keys {
                    cx.stats.props_seen.insert((false, node_id.0, k));
                }
                if let Some(alias) = node_alias {
                    row.insert(alias.0.clone(), Value::Node(node_id));
                }
                for item in on_create {
                    self.apply_set_item(cx.graph, &row, item, cx.stats)?;
                }
                cx.budget.record_row();
                out.push(row);
            }
        }
        self.invalidate_property_cache();
        Ok(out)
    }

    pub(crate) fn exec_merge_rel(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
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
            debug_assert!(false, "expected MergeRelationship operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected MergeRelationship operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let type_str = rel_type.as_str();
        if on_create.is_empty() && on_match.is_empty() {
            let out = self.exec_merge_rel_batch_creates(
                cx,
                input,
                MergeRelBatchSpec {
                    src_alias,
                    dst_alias,
                    rel_alias: rel_alias.as_ref(),
                    rel_type,
                    direction: *direction,
                    properties,
                },
            )?;
            self.invalidate_property_cache();
            return Ok(out);
        }
        let mut out = Vec::with_capacity(input.len());

        for mut row in input {
            cx.budget.check_or_err()?;
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
            let existing_edges = self
                .store
                .out_edges_by_type(cx.graph, real_src, &[type_str])?;
            let mut matching: Vec<_> = existing_edges
                .into_iter()
                .filter(|adj| adj.neighbor_id == real_dst)
                .collect();
            if matches!(direction, Direction::Undirected) && matching.is_empty() {
                // Also check the reverse direction (real_dst -> real_src).
                let reverse_edges =
                    self.store
                        .out_edges_by_type(cx.graph, real_dst, &[type_str])?;
                matching = reverse_edges
                    .into_iter()
                    .filter(|adj| adj.neighbor_id == real_src)
                    .collect();
            }

            // Filter by property constraints if any.
            let ctx = self.build_filter_ctx(cx.graph, &row)?;
            let prop_vals = self.eval_merge_properties(cx.graph, properties, &row, &ctx)?;
            let prop_map = Self::merge_relationship_props_map(prop_vals);

            let matched_edges: Vec<crate::storage::EdgeId> = if prop_map.is_empty() {
                matching.iter().map(|adj| adj.edge_id).collect::<Vec<_>>()
            } else {
                // Find ALL edges matching property constraints.
                let mut found_edges = Vec::new();
                for adj in &matching {
                    if let Some(edge_data) = self.store.get_edge(cx.graph, adj.edge_id)? {
                        let all_match = prop_map.iter().all(|(k, v)| {
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
                    cx.budget.check_or_err()?;
                    let mut new_row = row.clone();
                    if let Some(ra) = rel_alias {
                        new_row.insert(ra.0.clone(), Value::Edge(edge_id));
                    }
                    for item in on_match {
                        self.apply_set_item(cx.graph, &new_row, item, cx.stats)?;
                    }
                    cx.budget.record_row();
                    out.push(new_row);
                }
            } else {
                // NOT FOUND: create the edge with properties.
                let non_null_edge_keys: Vec<String> = prop_map
                    .iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, _)| k.clone())
                    .collect();
                let edge_id = self.store.create_edge(
                    cx.graph,
                    real_src,
                    real_dst,
                    type_str.to_string(),
                    prop_map,
                )?;
                cx.stats.edges_created += 1;
                for k in non_null_edge_keys {
                    cx.stats.props_seen.insert((true, edge_id.0, k));
                }
                if let Some(ra) = rel_alias {
                    row.insert(ra.0.clone(), Value::Edge(edge_id));
                }
                for item in on_create {
                    self.apply_set_item(cx.graph, &row, item, cx.stats)?;
                }
                cx.budget.record_row();
                out.push(row);
            }
        }
        self.invalidate_property_cache();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Direction, Expr, Identifier, Label, RelType};
    use crate::executor::budget::ExecutionBudget;
    use crate::planner::{Operator, SetItem};
    use crate::storage::{GraphStore, InMemoryGraphStore};

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
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
            .create_node("g", vec!["P".to_string()], IndexMap::new())
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

    #[test]
    fn merge_returns_all_matches_no_create() {
        let store = make_store();
        // Create 5 nodes with the same label.
        for _ in 0..5 {
            store
                .create_node("g", vec!["P".to_string()], IndexMap::new())
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

    #[test]
    fn merge_on_create_sets_property() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("P")],
            properties: vec![],
            on_create: vec![SetItem::Property {
                alias: Identifier::new("n"),
                key: "created".to_string(),
                value: Expr::Bool(true),
            }],
            on_match: vec![],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 1);
        let ids = store.find_nodes_by_label("g", "P").unwrap();
        let node = store.get_node("g", ids[0]).unwrap().unwrap();
        assert_eq!(node.properties.get("created"), Some(&Value::Bool(true)));
    }

    #[test]
    fn merge_on_match_sets_property() {
        let store = make_store();
        let nid = store
            .create_node("g", vec!["P".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("P")],
            properties: vec![],
            on_create: vec![],
            on_match: vec![SetItem::Property {
                alias: Identifier::new("n"),
                key: "matched".to_string(),
                value: Expr::Bool(true),
            }],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 0);
        let node = store.get_node("g", nid).unwrap().unwrap();
        assert_eq!(node.properties.get("matched"), Some(&Value::Bool(true)));
    }

    #[test]
    fn merge_with_properties_creates_matching_node() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("P")],
            properties: vec![("name".to_string(), Expr::Str("Alice".into()))],
            on_create: vec![],
            on_match: vec![],
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
    }

    #[test]
    fn merge_with_properties_finds_existing() {
        let store = make_store();
        let mut props = IndexMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        store
            .create_node("g", vec!["P".to_string()], props)
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("P")],
            properties: vec![("name".to_string(), Expr::Str("Alice".into()))],
            on_create: vec![],
            on_match: vec![],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 0);
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn merge_with_null_property_returns_error() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("P")],
            properties: vec![("name".to_string(), Expr::Null)],
            on_create: vec![],
            on_match: vec![],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget);
        assert!(
            result.is_err(),
            "MERGE with null property should be an error"
        );
    }

    #[test]
    fn merge_rel_creates_when_not_found() {
        let store = make_store();
        let _a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let _b = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::MergeRelationship {
            src_alias: Identifier::new("a"),
            dst_alias: Identifier::new("b"),
            rel_alias: Some(Identifier::new("r")),
            rel_type: RelType::new("KNOWS"),
            direction: Direction::Outgoing,
            properties: vec![],
            on_create: vec![],
            on_match: vec![],
            child: Box::new(Operator::Apply {
                outer: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("a"),
                }),
                inner: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("b"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // With 2 nodes, Apply produces rows for each (a,b) pair where a!=b
        // (since inner NodeScan with outer_rows only produces matching nodes)
        // Actually it produces all combinations. Let's just check edges were created.
        assert!(result.stats.edges_created > 0);
    }

    #[test]
    fn merge_rel_matches_existing() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["A".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["B".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), IndexMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::MergeRelationship {
            src_alias: Identifier::new("a"),
            dst_alias: Identifier::new("b"),
            rel_alias: Some(Identifier::new("r")),
            rel_type: RelType::new("R"),
            direction: Direction::Outgoing,
            properties: vec![],
            on_create: vec![],
            on_match: vec![],
            child: Box::new(Operator::Apply {
                outer: Box::new(Operator::NodeScan {
                    label: Some(Label::new("A")),
                    alias: Identifier::new("a"),
                }),
                inner: Box::new(Operator::NodeScan {
                    label: Some(Label::new("B")),
                    alias: Identifier::new("b"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(
            result.stats.edges_created, 0,
            "existing edge should be matched, not created"
        );
    }

    #[test]
    fn merge_rel_batch_path_deduplicates_duplicate_input_rows() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["A".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["B".to_string()], IndexMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::MergeRelationship {
            src_alias: Identifier::new("a"),
            dst_alias: Identifier::new("b"),
            rel_alias: Some(Identifier::new("r")),
            rel_type: RelType::new("R"),
            direction: Direction::Outgoing,
            properties: vec![],
            on_create: vec![],
            on_match: vec![],
            child: Box::new(Operator::Unwind {
                list_expr: Expr::List(vec![Expr::Integer(1), Expr::Integer(2)]),
                alias: Identifier::new("dup"),
                child: Box::new(Operator::Apply {
                    outer: Box::new(Operator::NodeByIdSeek {
                        id_expr: Expr::Integer(a.0 as i64),
                        alias: Identifier::new("a"),
                    }),
                    inner: Box::new(Operator::NodeByIdSeek {
                        id_expr: Expr::Integer(b.0 as i64),
                        alias: Identifier::new("b"),
                    }),
                }),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.edges_created, 1);
        assert_eq!(result.rows.len(), 2);

        let edge_ids = store.find_edges_by_type("g", "R").unwrap();
        assert_eq!(
            edge_ids.len(),
            1,
            "duplicate MERGE rows must reuse one edge"
        );

        let rel_values: Vec<_> = result
            .rows
            .iter()
            .map(|row| row.get("r").cloned().expect("merge should bind rel alias"))
            .collect();
        assert_eq!(rel_values[0], rel_values[1]);
    }

    #[test]
    fn merge_rel_fast_and_slow_paths_share_duplicate_key_semantics() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["A".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["B".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge(
                "g",
                a,
                b,
                "R".to_string(),
                IndexMap::from([("dup".to_string(), Value::Int(2))]),
            )
            .unwrap();

        let duplicate_key_props = vec![
            ("dup".to_string(), Expr::Integer(1)),
            ("dup".to_string(), Expr::Integer(2)),
        ];

        let fast_executor = QueryExecutor::new(&store);
        let fast_plan = Operator::MergeRelationship {
            src_alias: Identifier::new("a"),
            dst_alias: Identifier::new("b"),
            rel_alias: Some(Identifier::new("r")),
            rel_type: RelType::new("R"),
            direction: Direction::Outgoing,
            properties: duplicate_key_props.clone(),
            on_create: vec![],
            on_match: vec![],
            child: Box::new(Operator::Apply {
                outer: Box::new(Operator::NodeByIdSeek {
                    id_expr: Expr::Integer(a.0 as i64),
                    alias: Identifier::new("a"),
                }),
                inner: Box::new(Operator::NodeByIdSeek {
                    id_expr: Expr::Integer(b.0 as i64),
                    alias: Identifier::new("b"),
                }),
            }),
        };
        let mut fast_budget = ExecutionBudget::unlimited();
        let fast_result = fast_executor
            .execute("g", &fast_plan, &mut fast_budget)
            .unwrap();

        let slow_executor = QueryExecutor::new(&store);
        let slow_plan = Operator::MergeRelationship {
            src_alias: Identifier::new("a"),
            dst_alias: Identifier::new("b"),
            rel_alias: Some(Identifier::new("r")),
            rel_type: RelType::new("R"),
            direction: Direction::Outgoing,
            properties: duplicate_key_props,
            on_create: vec![crate::planner::SetItem::Property {
                alias: Identifier::new("r"),
                key: "seen".to_string(),
                value: Expr::Bool(true),
            }],
            on_match: vec![],
            child: Box::new(Operator::Apply {
                outer: Box::new(Operator::NodeByIdSeek {
                    id_expr: Expr::Integer(a.0 as i64),
                    alias: Identifier::new("a"),
                }),
                inner: Box::new(Operator::NodeByIdSeek {
                    id_expr: Expr::Integer(b.0 as i64),
                    alias: Identifier::new("b"),
                }),
            }),
        };
        let mut slow_budget = ExecutionBudget::unlimited();
        let slow_result = slow_executor
            .execute("g", &slow_plan, &mut slow_budget)
            .unwrap();

        assert_eq!(fast_result.stats.edges_created, 0);
        assert_eq!(slow_result.stats.edges_created, 0);
        assert_eq!(
            fast_result.rows[0].get("r"),
            slow_result.rows[0].get("r"),
            "fast and slow MERGE paths must match the same existing edge"
        );
    }
}
