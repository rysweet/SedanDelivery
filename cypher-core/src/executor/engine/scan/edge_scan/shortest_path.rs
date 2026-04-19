//! `ShortestPath` operator: BFS-based shortest / all-shortest paths.

use std::collections::{HashSet, VecDeque};

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::storage::NodeId;
use crate::types::Value;

use crate::executor::engine::helpers::{extract_node_id, get_adj_entries, undefined_var};
use crate::executor::engine::{BfsArena, ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_shortest_path(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::ShortestPath {
            src_alias,
            dst_alias,
            rel_types,
            direction,
            min: _min,
            max,
            path_alias,
            all_paths,
        } = op
        else {
            debug_assert!(false, "expected ShortestPath operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected ShortestPath operator".into(),
            }));
        };
        // ShortestPath requires both src and dst to be bound in outer context.
        // For now, work from outer_rows or require them to be bound.
        let type_strs: Vec<&str> = rel_types.iter().map(|r| r.as_str()).collect();
        let mut out = Vec::new();

        // If there are outer rows, use them. Otherwise create an empty row.
        let context_rows = if cx.outer_rows.is_empty() {
            vec![Row::new()]
        } else {
            cx.outer_rows.to_vec()
        };

        for row in &context_rows {
            cx.budget.check_or_err()?;
            let src_val = row
                .get(src_alias.0.as_str())
                .ok_or_else(|| undefined_var(&src_alias.0))?;
            let src_id = extract_node_id(src_val, &src_alias.0)?;

            let dst_val = row
                .get(dst_alias.0.as_str())
                .ok_or_else(|| undefined_var(&dst_alias.0))?;
            let dst_id = extract_node_id(dst_val, &dst_alias.0)?;

            // Arena-based BFS from src to dst.  Instead of cloning
            // Vec<NodeId> and Vec<EdgeId> per queue entry (O(depth) per step),
            // we use a parent-pointer arena -- O(1) per step.
            let (mut arena, root_idx) = BfsArena::new(src_id);

            // Queue: (current_node, arena_index, depth)
            let mut queue: VecDeque<(NodeId, usize, u32)> = VecDeque::new();
            queue.push_back((src_id, root_idx, 0));
            let mut visited = HashSet::new();
            visited.insert(src_id);

            // Store arena indices of found paths; reconstruct only at the end.
            let mut found_indices: Vec<usize> = Vec::new();
            let mut shortest_len: Option<u32> = None;

            while let Some((current, arena_idx, depth)) = queue.pop_front() {
                cx.budget.check_or_err()?;

                // If we already found shortest paths and current depth exceeds
                // that length, stop (BFS guarantees depth ordering).
                if let Some(sl) = shortest_len {
                    if depth > sl {
                        break;
                    }
                }

                // Check if we reached the destination
                if current == dst_id && depth > 0 {
                    found_indices.push(arena_idx);
                    shortest_len = Some(depth);
                    if !*all_paths {
                        // shortestPath: just need the first one
                        break;
                    }
                    // For allShortestPaths, continue at same depth level
                    continue;
                }

                // Don't expand beyond max depth
                if depth >= *max {
                    continue;
                }

                let adj_entries =
                    get_adj_entries(self.store, cx.graph, current, direction, &type_strs)?;
                for adj in adj_entries {
                    cx.budget.check_or_err()?;
                    // For shortest path BFS, we track visited globally
                    // but allow reaching dst through different paths at same depth.
                    let is_dst = adj.neighbor_id == dst_id;
                    if !is_dst && visited.contains(&adj.neighbor_id) {
                        continue;
                    }
                    if !is_dst {
                        visited.insert(adj.neighbor_id);
                    }
                    let child_idx = arena.push(adj.neighbor_id, adj.edge_id, arena_idx);
                    queue.push_back((adj.neighbor_id, child_idx, depth + 1));
                }
            }

            // Produce output rows for each found path, reconstructing only now.
            for idx in found_indices {
                let (pn, pe) = arena.reconstruct_path(idx);
                let mut new_row = row.clone();
                new_row.insert(
                    path_alias.0.clone(),
                    Value::Path {
                        nodes: pn,
                        edges: pe,
                    },
                );
                cx.budget.record_row();
                out.push(new_row);
            }
        }
        Ok(out)
    }
}
