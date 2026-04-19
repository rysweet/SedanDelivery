//! Edge traversal operators: Expand, VarLenExpand, ShortestPath.
//!
//! Each operator lives in its own submodule; tests are split by operator.

mod expand;
mod shortest_path;
mod var_len_expand;

#[cfg(test)]
mod tests;
