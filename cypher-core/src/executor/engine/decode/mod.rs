//! Path structure decoding: arena-based BFS tracking and path reconstruction.

use crate::storage::{EdgeId, NodeId};

/// Arena-allocated parent-pointer tree for BFS path tracking.
///
/// Instead of cloning `Vec<NodeId>`, `Vec<EdgeId>`, and `HashSet<EdgeId>` at
/// every BFS expansion step (O(depth) per step, O(d * b^d) total), we store a
/// flat arena of `(NodeId, EdgeId, parent_index)` entries.  Each BFS step
/// pushes one entry in O(1).  Full paths are reconstructed by walking parent
/// pointers only when producing output rows.
///
/// The root entry uses `parent == usize::MAX` as a sentinel.
pub(crate) struct BfsArena {
    /// Entries: `(node_id, edge_id, parent_index)`.
    /// Root entries have `parent == usize::MAX` and `edge_id == EdgeId(u64::MAX)` (unused).
    pub entries: Vec<(NodeId, EdgeId, usize)>,
}

impl BfsArena {
    /// Create a new arena with a root entry for the given source node.
    /// Returns `(arena, root_index)`.
    pub fn new(src: NodeId) -> (Self, usize) {
        let arena = BfsArena {
            entries: vec![(src, EdgeId(u64::MAX), usize::MAX)],
        };
        (arena, 0)
    }

    /// Push a new child entry and return its index.
    #[inline]
    pub fn push(&mut self, node: NodeId, edge: EdgeId, parent: usize) -> usize {
        let idx = self.entries.len();
        self.entries.push((node, edge, parent));
        idx
    }

    /// Check whether `edge` already appears on the path from `idx` back to root.
    /// Walks parent pointers -- O(depth) but depth is bounded by the query's `max`.
    #[inline]
    pub fn path_contains_edge(&self, mut idx: usize, edge: EdgeId) -> bool {
        while idx != usize::MAX {
            let (_, e, parent) = self.entries[idx];
            // Skip the root entry (sentinel edge)
            if parent != usize::MAX && e == edge {
                return true;
            }
            idx = parent;
        }
        false
    }

    /// Reconstruct the full path of nodes and edges from a given entry index
    /// back to the root, in forward (root-to-leaf) order.
    pub fn reconstruct_path(&self, mut idx: usize) -> (Vec<NodeId>, Vec<EdgeId>) {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        while idx != usize::MAX {
            let (node, edge, parent) = self.entries[idx];
            nodes.push(node);
            if parent != usize::MAX {
                edges.push(edge);
            }
            idx = parent;
        }
        nodes.reverse();
        edges.reverse();
        (nodes, edges)
    }
}

#[cfg(test)]
mod cmp_values_tests;
#[cfg(test)]
mod shortest_path_tests;
#[cfg(test)]
mod unwind_e2e_tests;
#[cfg(test)]
mod varlen_tests;
