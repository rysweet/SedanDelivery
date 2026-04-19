//! Per-row dispatcher for `MERGE (a)-[r:R]->(b)` that handles
//! `ON CREATE`/`ON MATCH` side-effect clauses and routes the
//! no-side-effect case to the batched fast-path in [`super::batch`].

use crate::ast::Direction;
use crate::error::{ExecutionError, GqlError};
use crate::executor::engine::helpers::{extract_node_id, undefined_var};
use crate::executor::engine::{ExecContext, QueryExecutor, Row};
use crate::planner::Operator;
use crate::types::Value;

use super::super::MergeRelBatchSpec;

impl<'a> QueryExecutor<'a> {
    /// Evaluate MERGE property expressions, expanding parameter maps and
    /// rejecting null values.  Shared by `exec_merge` and `exec_merge_rel`.
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
