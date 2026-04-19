//! Row-shaping operators: filter, project, distinct, sort, skip, limit.
//!
//! This module groups the row-shaping operators into focused submodules.
//! The public API (the `pub(crate)` `exec_*` methods on `QueryExecutor`)
//! is preserved exactly: each submodule contributes additional `impl`
//! blocks to the same `QueryExecutor` type.

mod distinct;
mod filter;
mod limit;
mod project;
mod skip;
mod sort;

#[cfg(test)]
mod tests;
