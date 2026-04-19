//! Gremlin "value-bag" function handlers split out of `eval`.
//!
//! Submodules group the handlers by Gremlin step:
//!
//! * [`valuemap`]    — `valueMap()` (wrapped properties, optional tokens)
//! * [`elementmap`]  — `elementMap()` (unwrapped properties + tokens + edge endpoints)
//! * [`values`]      — `values()` and `values(k1, k2, ...)`
//! * [`properties`]  — `properties()` (entries form with `~key`/`~value`/`~element`)
//!
//! Each submodule adds methods to [`super::super::super::QueryExecutor`] via
//! inherent `impl` blocks; visibility is `pub(in crate::executor::engine::eval)`
//! so the dispatcher in [`super::function_call`] can call them just like the
//! original flat module did.

mod elementmap;
mod properties;
mod valuemap;
mod values;
