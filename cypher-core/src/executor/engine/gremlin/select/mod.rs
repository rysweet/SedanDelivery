//! Gremlin executor: `select` and `match` step implementations.
//!
//! Decomposed into submodules grouped by select-step variant:
//! - [`match_step`] — `GremlinMatch` pattern join semantics.
//! - [`select_step`] — `GremlinSelect` orchestration and output building.
//! - [`pop`] — Pop modulator branch (first/last/all/mixed).
//! - [`prop_accessor`] — `__prop_PROPNAME__VARNAME` by-projection branch.
//! - [`lookup`] — Direct row/side-effect/expression fallback lookup.

mod lookup;
mod match_step;
mod pop;
mod prop_accessor;
mod select_step;

use crate::types::Value;

/// Outcome of trying one select-key resolution branch.
pub(super) enum SelectBranch {
    /// Branch did not apply; try the next one.
    NotApplicable,
    /// Branch produced a value; insert it for this key and continue.
    Insert(Value),
    /// Branch demands dropping the traverser entirely.
    Break,
}
