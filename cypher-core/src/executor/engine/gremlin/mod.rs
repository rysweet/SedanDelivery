//! Gremlin traversal operator implementations, decomposed by step category.
//!
//! Each submodule contributes one or more methods to `impl QueryExecutor<'a>`.
//! Pure mechanical refactor of the former monolithic `gremlin.rs`; no behavior changes.

mod helpers;

mod branch;
mod group;
mod path;
mod projection;
mod repeat;
mod select;
mod select_helpers;
mod side_effect;

#[cfg(test)]
mod tests;
