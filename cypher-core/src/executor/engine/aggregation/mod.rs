//! Aggregation operator and state for the query executor.
//!
//! The original `aggregation.rs` has been decomposed into per-family
//! submodules.  Public visibility is unchanged: `AggState` is still
//! `pub(crate)`, and `QueryExecutor::exec_aggregate` is contributed by
//! the `exec` submodule via an additional `impl` block.

mod collect;
mod count;
mod distinct;
mod exec;
mod group_key;
mod min_max;
mod percentile_stddev;
mod state;
mod sum_avg;

#[cfg(test)]
mod tests;
