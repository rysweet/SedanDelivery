//! Scan-related helper tests for the query executor engine.
//!
//! The tests are split into focused submodules grouped by the operator
//! category exercised:
//!
//! * [`common`] — shared fixtures (e.g. [`common::make_store`])
//! * [`scan`] — `NodeScan`, `Filter`, `Expand`
//! * [`mutations`] — `Create`, `Set`, `Remove`
//! * [`delete`] — `Delete` (detach, null, edges)
//! * [`merge`] — `Merge` (create / match / without child)
//! * [`union_apply`] — `Union`, `Apply`, `LeftOuterApply`
//! * [`seek`] — `IndexScan`, `NodeByIdSeek`

#[cfg(test)]
mod common;
#[cfg(test)]
mod delete;
#[cfg(test)]
mod merge;
#[cfg(test)]
mod mutations;
#[cfg(test)]
mod scan;
#[cfg(test)]
mod seek;
#[cfg(test)]
mod union_apply;
