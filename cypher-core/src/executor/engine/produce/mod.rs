//! Output and stream operators: exec_produce_results, exec_call, exec_assemble_path,
//! exec_unwind, exec_union.
//!
//! This module groups the produce/output operators into focused submodules.
//! The public API (the `pub(crate)` `exec_*` methods on `QueryExecutor`)
//! is preserved exactly: each submodule contributes additional `impl`
//! blocks to the same `QueryExecutor` type.

mod call;
mod path;
mod results;
mod union;
mod unwind;

#[cfg(test)]
mod tests;
