//! Per-step compilation submodules for `compile_from`.
//!
//! `dispatch.rs` is a thin top-level `match` that routes each
//! `GremlinStep` variant to one of the per-category submodules below.
//! Each submodule exposes a single `pub(super) fn compile_<category>_step`
//! method on `GremlinCompiler` that contains the verbatim arm bodies
//! formerly inlined in the giant `match`. The split is mechanical —
//! no behavior changes.

mod aggregation;
mod branch;
mod by;
mod dispatch;
mod filter;
mod limit;
mod misc;
mod mutation;
mod order;
mod path;
mod projection;
mod set_ops;
mod side_effect;
mod source;
mod strings;
mod traversal;

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
