//! `Expand` operator: single-step relationship traversal.

use std::collections::{HashMap, HashSet};

use crate::ast::Direction;
use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::storage::{EdgeId, NodeId};
use crate::types::Value;

use crate::executor::engine::helpers::{extract_node_id, undefined_var};
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

const EXPAND_BATCH_NODE_CHUNK_SIZE: usize = 256;

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_expand(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Expand {
            direction,
            rel_types,
            src_alias,
            rel_alias,
            dst_alias,
            child,
        } = op
        else {
            debug_assert!(false, "expected Expand operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Expand operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let type_strs: Vec<&str> = rel_types.iter().map(|r| r.as_str()).collect();
        let mut out = Vec::with_capacity(input.len());
        let mut row_src_ids = Vec::with_capacity(input.len());
        let mut unique_src_ids = Vec::new();
        let mut seen_src_ids = HashSet::new();

        for row in &input {
            cx.budget.check_or_err()?;
            let src_val = row
                .get(src_alias.0.as_str())
                .ok_or_else(|| undefined_var(&src_alias.0))?;
            let src_id = extract_node_id(src_val, &src_alias.0)?;
            row_src_ids.push(src_id);
            if seen_src_ids.insert(src_id) {
                unique_src_ids.push(src_id);
            }
        }

        let mut adjacency_by_src: HashMap<NodeId, Vec<crate::storage::AdjEntry>> =
            HashMap::with_capacity(unique_src_ids.len());
        for node_chunk in unique_src_ids.chunks(EXPAND_BATCH_NODE_CHUNK_SIZE) {
            cx.budget.check_or_err()?;
            let chunk_batches = match direction {
                Direction::Outgoing => self
                    .store
                    .out_edges_by_type_batch(cx.graph, node_chunk, &type_strs)?,
                Direction::Incoming => self
                    .store
                    .in_edges_by_type_batch(cx.graph, node_chunk, &type_strs)?,
                Direction::Undirected => {
                    let mut out_batches = self
                        .store
                        .out_edges_by_type_batch(cx.graph, node_chunk, &type_strs)?;
                    let in_batches = self
                        .store
                        .in_edges_by_type_batch(cx.graph, node_chunk, &type_strs)?;
                    for ((&src_node, out_entries), in_entries) in node_chunk
                        .iter()
                        .zip(out_batches.iter_mut())
                        .zip(in_batches)
                    {
                        let seen_out_edges: HashSet<EdgeId> =
                            out_entries.iter().map(|entry| entry.edge_id).collect();
                        let gremlin = self.gremlin_mode;
                        out_entries.extend(in_entries.into_iter().filter(|entry| {
                            // In Gremlin mode, self-loop edges (neighbor == src)
                            // must appear twice in bothE/both: once from
                            // outgoing, once from incoming. In Cypher/GQL mode,
                            // self-loops are deduped like other edges.
                            (gremlin && entry.neighbor_id == src_node)
                                || !seen_out_edges.contains(&entry.edge_id)
                        }));
                        if self.gremlin_mode {
                            out_entries.sort_by(|left, right| left.rel_type.cmp(&right.rel_type));
                        }
                    }
                    out_batches
                }
            };
            for (src_id, adj_entries) in node_chunk.iter().copied().zip(chunk_batches) {
                adjacency_by_src.insert(src_id, adj_entries);
            }
        }

        for (row, src_id) in input.into_iter().zip(row_src_ids) {
            cx.budget.check_or_err()?;
            let empty_adj: Vec<_> = Vec::new();
            let adj_entries = adjacency_by_src.get(&src_id).unwrap_or(&empty_adj);

            // Hoist outer_edge_vars construction to the per-row level.
            // This HashSet only depends on the row (not on individual adjacency
            // entries), so building it once per row avoids redundant allocations.
            let current_alias = rel_alias.as_ref().map(|ra| ra.0.as_str());
            let outer_edge_vars: HashSet<&str> = match row.get("__outer_edge_vars__") {
                Some(Value::List(vs)) => vs
                    .iter()
                    .filter_map(|v| {
                        if let Value::Str(s) = v {
                            Some(s.as_ref())
                        } else {
                            None
                        }
                    })
                    .collect(),
                _ => HashSet::new(),
            };

            for adj in adj_entries {
                cx.budget.check_or_err()?;
                // If dst_alias is already bound in the row (cyclic patterns
                // like (a)-->()-->(a)), only keep the result if the bound
                // value matches the expansion result.
                // Skip this check for anonymous variables: they may be
                // inherited from an outer scope (e.g. pattern comprehension)
                // and should not constrain the inner expansion.
                let dst_val = Value::Node(adj.neighbor_id);
                if !dst_alias.0.starts_with("__anon") {
                    if let Some(existing) = row.get(dst_alias.0.as_str()) {
                        if *existing != dst_val {
                            continue;
                        }
                    }
                }
                // Same for rel_alias: if already bound, values must match.
                if let Some(ra) = rel_alias {
                    let rel_val = Value::Edge(adj.edge_id);
                    if let Some(existing) = row.get(ra.0.as_str()) {
                        if *existing != rel_val {
                            continue;
                        }
                    }
                }
                // Edge isomorphism: per openCypher spec, each relationship in a
                // pattern can only be bound once in any single path traversal.
                // Skip this edge if it is already used by a DIFFERENT rel in the row
                // (exclude the current rel_alias, which may legitimately re-bind it).
                // Note: named path variables (Value::Path) are NOT checked here because
                // edges from paths in previous MATCH clauses can be reused independently.
                // Also exclude outer-scope edge variables (from previous WITH/MATCH scopes).
                //
                // In Gremlin mode, edge isomorphism is disabled — Gremlin traversals
                // allow revisiting the same edge from different directions.
                let eid = adj.edge_id;
                let edge_already_used = if self.gremlin_mode {
                    false
                } else {
                    row.iter().any(|(key, v)| {
                        // Don't count the current alias as "already used"
                        if Some(key.as_str()) == current_alias {
                            return false;
                        }
                        // Don't count outer-scope edges (from previous MATCH clauses)
                        if outer_edge_vars.contains(key.as_str()) {
                            return false;
                        }
                        match v {
                            Value::Edge(e) => *e == eid,
                            Value::List(items) => items
                                .iter()
                                .any(|item| matches!(item, Value::Edge(e) if *e == eid)),
                            // Named path variables are NOT subject to edge isomorphism
                            // across MATCH clauses — only relationship variables are.
                            Value::Path { .. } => false,
                            _ => false,
                        }
                    })
                };
                if edge_already_used {
                    continue;
                }
                let mut new_row = row.clone();
                if let Some(ra) = rel_alias {
                    new_row.insert(ra.0.clone(), Value::Edge(adj.edge_id));
                }
                new_row.insert(dst_alias.0.clone(), dst_val);
                cx.budget.record_row();
                out.push(new_row);
            }
        }
        Ok(out)
    }
}
