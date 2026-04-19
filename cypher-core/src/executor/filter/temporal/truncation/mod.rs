//! Temporal truncation functions (date.truncate, datetime.truncate, etc.)
//!
//! Submodules:
//! - [`extract`]: extract date/time/timezone components from temporal values.
//! - [`units`]: per-unit truncation logic (year, month, week, hour, …).
//! - [`overrides`]: apply map-shaped field overrides and value conversions.
//! - [`eval`]: public `eval_*_truncate` entry points used by the temporal builtins.

mod eval;
mod extract;
mod overrides;
mod units;

#[cfg(test)]
mod tests;

pub(crate) use eval::{
    eval_date_truncate, eval_datetime_truncate, eval_localdatetime_truncate,
    eval_localtime_truncate, eval_time_truncate,
};
