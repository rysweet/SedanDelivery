//! `MERGE (n {...})` node match-or-create implementation.

use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

use crate::error::{ExecutionError, GqlError, StorageError};
use crate::executor::engine::helpers::inject_parameters;
use crate::executor::engine::{ExecContext, QueryExecutor, Row};
use crate::planner::Operator;
use crate::storage::NodeId;
use crate::types::Value;

use super::{PendingMergeNodeKey, PendingMergeNodeSlot};

impl<'a> QueryExecutor<'a> {
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

                // Try indexed lookup; fall back to label scan if no index.
                let (candidates, skip_first_prop): (HashSet<u64>, bool) = if labels.is_empty() {
                    (
                        self.store
                            .all_node_ids(cx.graph)?
                            .into_iter()
                            .map(|n| n.0)
                            .collect(),
                        false,
                    )
                } else if !prop_vals.is_empty() {
                    let first_label = labels[0].as_str();
                    let (first_prop_key, first_prop_val) = &prop_vals[0];
                    match self.store.find_nodes_by_label_and_property(
                        cx.graph,
                        first_label,
                        first_prop_key,
                        crate::storage::RangeQuery::Eq(first_prop_val),
                    ) {
                        Ok(indexed_ids) => {
                            let mut cands: HashSet<u64> =
                                indexed_ids.into_iter().map(|n| n.0).collect();
                            for label in &labels[1..] {
                                let ids =
                                    self.store.find_nodes_by_label(cx.graph, label.as_str())?;
                                let id_set: HashSet<u64> = ids.iter().map(|n| n.0).collect();
                                cands = cands.intersection(&id_set).copied().collect();
                            }
                            (cands, true)
                        }
                        Err(StorageError::NoPropertyIndex { .. }) => {
                            // No index: fall back to label scan; check ALL props below.
                            let mut cands: Option<HashSet<u64>> = None;
                            for label in labels {
                                let ids =
                                    self.store.find_nodes_by_label(cx.graph, label.as_str())?;
                                let id_set: HashSet<u64> = ids.iter().map(|n| n.0).collect();
                                cands = Some(match cands {
                                    None => id_set,
                                    Some(existing) => {
                                        existing.intersection(&id_set).copied().collect()
                                    }
                                });
                            }
                            (cands.unwrap_or_default(), false)
                        }
                        Err(e) => return Err(e.into()),
                    }
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
                    (cands.unwrap_or_default(), false)
                };
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
            // Fast path: use index when available; explicit label scan otherwise.
            let (candidates, skip_first_prop): (HashSet<u64>, bool) = if labels.is_empty() {
                (
                    self.store
                        .all_node_ids(cx.graph)?
                        .into_iter()
                        .map(|n| n.0)
                        .collect(),
                    false,
                )
            } else if !prop_vals.is_empty() {
                let first_label = labels[0].as_str();
                let (first_prop_key, first_prop_val) = &prop_vals[0];
                match self.store.find_nodes_by_label_and_property(
                    cx.graph,
                    first_label,
                    first_prop_key,
                    crate::storage::RangeQuery::Eq(first_prop_val),
                ) {
                    Ok(indexed_ids) => {
                        let mut cands: HashSet<u64> =
                            indexed_ids.into_iter().map(|n| n.0).collect();
                        for label in &labels[1..] {
                            let ids = self.store.find_nodes_by_label(cx.graph, label.as_str())?;
                            let id_set: HashSet<u64> = ids.iter().map(|n| n.0).collect();
                            cands = cands.intersection(&id_set).copied().collect();
                        }
                        (cands, true)
                    }
                    Err(StorageError::NoPropertyIndex { .. }) => {
                        // No index: fall back to label scan; check ALL props below.
                        let mut cands: Option<HashSet<u64>> = None;
                        for label in labels {
                            let ids = self.store.find_nodes_by_label(cx.graph, label.as_str())?;
                            let id_set: HashSet<u64> = ids.iter().map(|n| n.0).collect();
                            cands = Some(match cands {
                                None => id_set,
                                Some(existing) => existing.intersection(&id_set).copied().collect(),
                            });
                        }
                        (cands.unwrap_or_default(), false)
                    }
                    Err(e) => return Err(e.into()),
                }
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
                (cands.unwrap_or_default(), false)
            };

            // Check candidates for matching nodes. MERGE returns one row per
            // match; if no matches are found, it creates a new node instead.
            // When the indexed fast-path was used, candidates are already filtered
            // by the first property — only verify remaining properties.
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
}
