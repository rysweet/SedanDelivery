//! Per-step-category dispatch for the Gremlin text parser.
//!
//! [`parse_step_token`] is a thin dispatcher that routes each step name to
//! the appropriate per-category submodule. Each submodule exposes a
//! `try_dispatch` function that returns `Ok(true)` if it handled the step,
//! or `Ok(false)` to fall through.

use super::tokenizer::*;
use super::*;

mod aggregation;
mod branch;
mod collection;
mod filter;
mod limit;
mod misc;
mod mutation;
mod path;
mod projection;
mod source;
mod strings;
mod traversal_steps;

#[cfg(test)]
mod tests;

// Re-export (currently used only within the steps subtree, but kept
// available crate-wide for future direct callers / tests).
#[allow(unused_imports)]
pub(crate) use filter::parse_has_step;
#[allow(unused_imports)]
pub(crate) use limit::parse_by_step;

pub(crate) fn parse_step_token(
    token: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<(), TextParseError> {
    let (name, args_str) = split_step_name_args(token);

    // Order is by frequency / category. No two categories accept the same
    // step name, so the order only affects performance, not behavior.
    if source::try_dispatch(name, args_str, resolve_param, out)?
        || traversal_steps::try_dispatch(name, args_str, resolve_param, out)?
        || filter::try_dispatch(name, args_str, resolve_param, out)?
        || projection::try_dispatch(name, args_str, resolve_param, out)?
        || mutation::try_dispatch(name, args_str, resolve_param, out)?
        || aggregation::try_dispatch(name, args_str, resolve_param, out)?
        || branch::try_dispatch(name, args_str, resolve_param, out)?
        || limit::try_dispatch(name, args_str, resolve_param, out)?
        || path::try_dispatch(name, args_str, resolve_param, out)?
        || strings::try_dispatch(name, args_str, resolve_param, out)?
        || collection::try_dispatch(name, args_str, resolve_param, out)?
        || misc::try_dispatch(name, args_str, resolve_param, out)?
    {
        return Ok(());
    }

    Err(TextParseError::UnknownStep(name.to_string()))
}
