//! Temporal Bolt structure decoders.
//!
//! Decodes Bolt Date, LocalTime, Time, LocalDateTime, DateTimeZoneId,
//! DateTimeOffset, and Duration structures into cypher-core `Value` types.

mod date;
mod datetime;
mod duration;
mod time;

use crate::packstream::value::BoltValue;

pub(crate) use date::decode_date;
pub(crate) use datetime::{decode_datetime_offset, decode_datetime_zone_id, decode_local_datetime};
pub(crate) use duration::decode_duration;
pub(crate) use time::{decode_local_time, decode_time_with_offset};

/// Extract an i64 from a BoltValue, returning None if it is not an Int.
pub(crate) fn bolt_int(v: &BoltValue) -> Option<i64> {
    match v {
        BoltValue::Int(i) => Some(*i),
        _ => None,
    }
}
