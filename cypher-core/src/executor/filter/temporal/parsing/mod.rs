//! ISO 8601 parsing, compact datetime format parsing, proleptic Gregorian calendar helpers,
//! and large-year date sentinel handling.
//!
//! This module is decomposed by topic:
//! - [`large_year`]: large-year date/datetime sentinel values and proleptic Gregorian calendar helpers
//! - [`date`]: ISO 8601 date string parsing (week-date, ordinal-date, compact)
//! - [`time`]: time-string helpers (compact-time expansion, offset stripping)
//! - [`timezone`]: timezone offset extraction from datetime strings
//! - [`datetime`]: ISO 8601 datetime parsing with offsets/timezones
//! - [`duration`]: ISO 8601 duration parsing with decimal cascade and calendar format

mod date;
mod datetime;
mod duration;
mod large_year;
mod time;
mod timezone;

pub(crate) use date::parse_date_str;
pub(crate) use datetime::{parse_datetime_with_offset, parse_localdatetime_str};
pub(crate) use duration::parse_duration_str;
pub(crate) use large_year::{
    extract_large_date, extract_large_ldt, large_date_duration_between, large_date_value,
    large_ldt_in_seconds, large_ldt_value, parse_large_date_str,
};
pub(crate) use time::{expand_compact_time, strip_tz_offset_from_time};
pub(crate) use timezone::extract_offset_from_string;
