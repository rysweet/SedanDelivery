//! Batched fast-path for `MERGE (a)-[r:R]->(b)` without `ON CREATE`/`ON MATCH`
//! clauses.  Deduplicates identical pending creates within a single MERGE
//! pipeline and uses batched storage reads for adjacency and edge property
//! lookups.

use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

use crate::ast::Direction;
use crate::error::GqlError;
use crate::executor::engine::helpers::{extract_node_id, undefined_var};
use crate::executor::engine::{ExecContext, QueryExecutor, Row};
use crate::storage::{AdjEntry, EdgeId, NodeId};
use crate::types::Value;

use super::super::{MergeRelBatchSpec, PendingMergeEdgeKey, PendingMergeEdgeSlot};

impl<'a> QueryExecutor<'a> {
    pub(in crate::executor::engine::mutations::merge) fn exec_merge_rel_batch_creates(
        &self,
        cx: &mut ExecContext<'_>,
        input: Vec<Row>,
        spec: MergeRelBatchSpec<'_>,
    ) -> Result<Vec<Row>, GqlError> {
        let type_str = spec.rel_type.as_str();
        let type_owned = type_str.to_string();
        let is_undirected = matches!(spec.direction, Direction::Undirected);

        // ── Phase 1: Evaluate all rows, collect specs and dedup keys ─────────
        struct RowSpec {
            row: Row,
            real_src: NodeId,
            real_dst: NodeId,
            prop_map: IndexMap<String, Value>,
            dedup_key: PendingMergeEdgeKey,
        }

        let mut row_specs = Vec::with_capacity(input.len());
        let mut unique_src_set: HashSet<NodeId> = HashSet::new();

        for row in input {
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
                src: if is_undirected && real_src.0 > real_dst.0 {
                    real_dst
                } else {
                    real_src
                },
                dst: if is_undirected && real_src.0 > real_dst.0 {
                    real_src
                } else {
                    real_dst
                },
                rel_type: type_owned.clone(),
                props_key: Self::merge_relationship_props_key(&prop_map),
            };

            unique_src_set.insert(real_src);
            row_specs.push(RowSpec {
                row,
                real_src,
                real_dst,
                prop_map,
                dedup_key,
            });
        }

        // ── Phase 2: Batch-fetch forward adjacency for all unique sources ────
        let unique_srcs: Vec<NodeId> = unique_src_set.iter().copied().collect();
        let batch_adj = self
            .store
            .out_edges_by_type_batch(cx.graph, &unique_srcs, &[type_str])?;
        let mut adj_by_src: HashMap<NodeId, Vec<AdjEntry>> =
            HashMap::with_capacity(unique_srcs.len());
        for (i, adjs) in batch_adj.into_iter().enumerate() {
            adj_by_src.insert(unique_srcs[i], adjs);
        }

        // ── Phase 3: First resolution pass (forward edges) ──────────────────
        // For each unique dedup_key, find matching adjacency entries.
        // Track which undirected keys need a reverse lookup.
        let mut pending_creates: Vec<(NodeId, NodeId, String, IndexMap<String, Value>)> =
            Vec::new();
        let mut pending_non_null_keys: Vec<Vec<String>> = Vec::new();
        let mut pending_by_key: HashMap<PendingMergeEdgeKey, usize> = HashMap::new();
        // Maps dedup_key index -> matching AdjEntry list (forward pass).
        // None means "needs reverse check" (undirected only).
        struct UniqueKeyResult {
            real_src: NodeId,
            real_dst: NodeId,
            prop_map: IndexMap<String, Value>,
            forward_matching: Vec<AdjEntry>,
        }
        let mut unique_key_results: Vec<UniqueKeyResult> = Vec::new();
        let mut unique_key_index: HashMap<PendingMergeEdgeKey, usize> = HashMap::new();
        let mut needs_reverse: Vec<usize> = Vec::new();

        for rs in &row_specs {
            if unique_key_index.contains_key(&rs.dedup_key) {
                continue;
            }
            let idx = unique_key_results.len();
            unique_key_index.insert(rs.dedup_key.clone(), idx);

            let empty = Vec::new();
            let src_adjs = adj_by_src.get(&rs.real_src).unwrap_or(&empty);
            let forward_matching: Vec<AdjEntry> = src_adjs
                .iter()
                .filter(|adj| adj.neighbor_id == rs.real_dst)
                .cloned()
                .collect();

            if is_undirected && forward_matching.is_empty() {
                needs_reverse.push(idx);
            }

            unique_key_results.push(UniqueKeyResult {
                real_src: rs.real_src,
                real_dst: rs.real_dst,
                prop_map: rs.prop_map.clone(),
                forward_matching,
            });
        }

        // ── Phase 4: Batch-fetch reverse adjacency for undirected misses ─────
        if !needs_reverse.is_empty() {
            // Collect unique reverse-source nodes (real_dst of rows that had no forward match).
            let mut reverse_src_set: HashSet<NodeId> = HashSet::new();
            for &idx in &needs_reverse {
                reverse_src_set.insert(unique_key_results[idx].real_dst);
            }
            // Only fetch nodes we haven't already fetched.
            let reverse_srcs: Vec<NodeId> = reverse_src_set
                .into_iter()
                .filter(|nid| !adj_by_src.contains_key(nid))
                .collect();
            if !reverse_srcs.is_empty() {
                let reverse_batch =
                    self.store
                        .out_edges_by_type_batch(cx.graph, &reverse_srcs, &[type_str])?;
                for (i, adjs) in reverse_batch.into_iter().enumerate() {
                    adj_by_src.insert(reverse_srcs[i], adjs);
                }
            }
            // Now resolve reverse matches.
            for &idx in &needs_reverse {
                let ukr = &mut unique_key_results[idx];
                if !ukr.forward_matching.is_empty() {
                    continue;
                }
                let empty = Vec::new();
                let dst_adjs = adj_by_src.get(&ukr.real_dst).unwrap_or(&empty);
                let reverse_matching: Vec<AdjEntry> = dst_adjs
                    .iter()
                    .filter(|adj| adj.neighbor_id == ukr.real_src)
                    .cloned()
                    .collect();
                ukr.forward_matching = reverse_matching;
            }
        }

        // ── Phase 5: Property-match pass with batched edge reads ─────────────
        // For unique keys with non-empty props, collect edge IDs that need
        // property verification, then batch-fetch them.
        let has_any_props = row_specs.iter().any(|rs| !rs.prop_map.is_empty());

        // Maps unique_key_index -> final matched EdgeIds.
        let mut matched_edges_by_key: Vec<Vec<EdgeId>> =
            Vec::with_capacity(unique_key_results.len());

        if has_any_props {
            // Collect all candidate edge IDs across all unique keys that have props.
            let mut all_candidate_edge_ids: Vec<EdgeId> = Vec::new();
            // For each unique key, store (start, count) into all_candidate_edge_ids.
            let mut candidate_ranges: Vec<(usize, usize)> =
                Vec::with_capacity(unique_key_results.len());

            for ukr in &unique_key_results {
                if ukr.prop_map.is_empty() {
                    // No property filter needed; all matching edges are valid.
                    candidate_ranges.push((0, 0)); // sentinel: empty prop_map
                } else {
                    let start = all_candidate_edge_ids.len();
                    for adj in &ukr.forward_matching {
                        all_candidate_edge_ids.push(adj.edge_id);
                    }
                    candidate_ranges.push((start, ukr.forward_matching.len()));
                }
            }

            // Batch-fetch all candidate edges at once.
            let all_edge_data = if all_candidate_edge_ids.is_empty() {
                Vec::new()
            } else {
                self.store
                    .get_edges_batch(cx.graph, &all_candidate_edge_ids)?
            };

            // Resolve matches for each unique key.
            for (ki, ukr) in unique_key_results.iter().enumerate() {
                if ukr.prop_map.is_empty() {
                    matched_edges_by_key
                        .push(ukr.forward_matching.iter().map(|adj| adj.edge_id).collect());
                } else {
                    let (start, count) = candidate_ranges[ki];
                    let mut found = Vec::new();
                    for i in start..start + count {
                        if let Some(edge_data) = &all_edge_data[i] {
                            let all_match = ukr.prop_map.iter().all(|(k, v)| {
                                edge_data
                                    .properties
                                    .get(k.as_str())
                                    .map(|ev| ev == v)
                                    .unwrap_or(false)
                            });
                            if all_match {
                                found.push(all_candidate_edge_ids[i]);
                            }
                        }
                    }
                    matched_edges_by_key.push(found);
                }
            }
        } else {
            // Fast path: no properties to check.
            for ukr in &unique_key_results {
                matched_edges_by_key
                    .push(ukr.forward_matching.iter().map(|adj| adj.edge_id).collect());
            }
        }

        // ── Phase 6: Build slots (same logic as before, using cached results) ─
        let mut slots = Vec::with_capacity(row_specs.len());

        for rs in row_specs {
            if let Some(&create_index) = pending_by_key.get(&rs.dedup_key) {
                slots.push(PendingMergeEdgeSlot::Pending {
                    row: rs.row,
                    create_index,
                });
                continue;
            }

            let ki = unique_key_index[&rs.dedup_key];
            let matched = &matched_edges_by_key[ki];

            if matched.is_empty() {
                let create_index = pending_creates.len();
                pending_non_null_keys.push(
                    rs.prop_map
                        .iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(k, _)| k.clone())
                        .collect(),
                );
                pending_creates.push((rs.real_src, rs.real_dst, type_str.to_string(), rs.prop_map));
                pending_by_key.insert(rs.dedup_key, create_index);
                slots.push(PendingMergeEdgeSlot::Pending {
                    row: rs.row,
                    create_index,
                });
            } else {
                let mut ready_rows = Vec::with_capacity(matched.len());
                let mut row = rs.row;
                for &edge_id in matched {
                    cx.budget.check_or_err()?;
                    if let Some(alias) = spec.rel_alias {
                        row.insert(alias.0.clone(), Value::Edge(edge_id));
                    }
                    cx.budget.record_row();
                    ready_rows.push(row.clone());
                }
                slots.push(PendingMergeEdgeSlot::Ready(ready_rows));
            }
        }

        // ── Phase 7: Batch-create missing edges ─────────────────────────────
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
}
