//! Per-step compilation submodules for `compile_from`.
//!
//! Currently hosts a single `dispatch` module that contains the giant
//! `match step { ... }` extracted from `compile_from.rs`. The split is
//! mechanical — no behavior changes — so further per-arm-group splits
//! (filter/traversal/projection/etc.) can be done incrementally without
//! disturbing the outer compilation loop.

mod dispatch;

/// Control-flow signal returned by [`super::GremlinCompiler::dispatch_step`].
///
/// Mirrors the bare `continue;` statements that previously appeared
/// inside the giant outer-loop `match`. Returning `Continue` tells the
/// caller to skip the post-step path-tracking bookkeeping for this
/// iteration; returning `Proceed` means the caller should run it.
pub(super) enum StepFlow {
    Continue,
    Proceed,
}
