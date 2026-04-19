//! Graph traversal primitives used by the executor.
//!
//! Currently this module exposes [`BfsTraversal`], a breadth-first iterator
//! over a [`crate::storage::GraphStore`].  Future depth-first or bidirectional
//! traversals would live as sibling submodules and be re-exported here.

mod bfs;

#[cfg(test)]
mod bfs_tests;

pub use bfs::BfsTraversal;
