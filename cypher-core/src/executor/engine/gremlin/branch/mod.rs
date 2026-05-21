//! Gremlin executor: `branch` step implementations.
//!
//! - [`choose`] — `choose()` predicate/value-based dispatch (`Choose`, `ChooseMap` operators).
//! - [`coalesce`] — `coalesce()` first-match alternative traversal.

mod choose;
mod coalesce;
