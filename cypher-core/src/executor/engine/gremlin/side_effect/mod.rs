//! Gremlin executor: `side_effect` step implementations.
//!
//! Grouped by side-effect category:
//!
//! - [`passthrough`] — `side_effect()` step that runs an inner traversal for
//!   its effects and passes input rows through unchanged.
//! - [`aggregate`] — `aggregate()` / `store()` / `group()` accumulation into
//!   the side-effect store under `__gremlin_agg_<key>` columns.
//! - [`cap`] — `cap()` step that emits accumulated side-effect values.

mod aggregate;
mod cap;
mod passthrough;
