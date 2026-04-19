//! Bolt LocalDateTime, DateTimeZoneId and DateTimeOffset structure decoders.

use cypher_core::types::Value;

use super::bolt_int;
use crate::packstream::value::BoltValue;

/// LocalDateTime (tag 0x64): 2 fields -- epoch_seconds, nanosecond_adjustment.
pub(crate) fn decode_local_datetime(fields: &[BoltValue]) -> Value {
    let epoch_secs = match fields.first().and_then(bolt_int) {
        Some(s) => s,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'epoch_seconds' field in LocalDateTime structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let nanos_adj = fields.get(1).and_then(bolt_int).unwrap_or(0);

    // Decompose epoch_secs into days + time-of-day seconds.
    // Handle negative epoch_secs correctly (dates before 1970).
    let (days, day_secs) = if epoch_secs >= 0 {
        (epoch_secs / 86_400, epoch_secs % 86_400)
    } else {
        // Floor division for negative values.
        let d = (epoch_secs - 86_399) / 86_400;
        let s = epoch_secs - d * 86_400;
        (d, s)
    };

    let epoch = jiff::civil::Date::new(1970, 1, 1).expect("valid epoch");
    let date = match epoch.checked_add(jiff::Span::new().days(days)) {
        Ok(d) => d,
        Err(_) => {
            tracing::warn!(
                "bolt_decode: LocalDateTime date overflow, epoch_secs={epoch_secs}, days={days}"
            );
            return Value::Null;
        }
    };

    let h = (day_secs / 3_600) as i8;
    let rem = day_secs % 3_600;
    let m = (rem / 60) as i8;
    let s = (rem % 60) as i8;

    let nanos_i32 = match i32::try_from(nanos_adj) {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!(
                "bolt_decode: nanos_adj={nanos_adj} overflows i32 in LocalDateTime structure"
            );
            return Value::Null;
        }
    };

    let time = match jiff::civil::Time::new(h, m, s, nanos_i32) {
        Ok(t) => t,
        Err(_) => {
            tracing::warn!("bolt_decode: out-of-range LocalDateTime time component, epoch_secs={epoch_secs}, nanos_adj={nanos_adj}");
            return Value::Null;
        }
    };

    Value::LocalDateTime(jiff::civil::DateTime::from_parts(date, time))
}

/// DateTime with zone ID (tag 0x49/0x69): 3 fields -- epoch_secs, nanos, tz_id string.
pub(crate) fn decode_datetime_zone_id(fields: &[BoltValue]) -> Value {
    let epoch_secs = match fields.first().and_then(bolt_int) {
        Some(s) => s,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'epoch_seconds' field in DateTimeZoneId structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let nanos_adj = fields.get(1).and_then(bolt_int).unwrap_or(0);
    let tz_id = match fields.get(2) {
        Some(BoltValue::String(s)) => s.as_str(),
        _ => {
            tracing::warn!("bolt_decode: missing or non-string 'tz_id' field in DateTimeZoneId structure, fields={fields:?}");
            return Value::Null;
        }
    };

    let tz = match jiff::tz::TimeZone::get(tz_id) {
        Ok(tz) => tz,
        Err(_) => {
            tracing::warn!(
                "bolt_decode: invalid timezone ID '{tz_id}' in DateTimeZoneId structure"
            );
            return Value::Null;
        }
    };

    let nanos_i32 = match i32::try_from(nanos_adj) {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!(
                "bolt_decode: nanos_adj={nanos_adj} overflows i32 in DateTimeZoneId structure"
            );
            return Value::Null;
        }
    };

    let ts = match jiff::Timestamp::new(epoch_secs, nanos_i32) {
        Ok(ts) => ts,
        Err(_) => {
            tracing::warn!("bolt_decode: out-of-range timestamp in DateTimeZoneId, epoch_secs={epoch_secs}, nanos_adj={nanos_adj}");
            return Value::Null;
        }
    };

    let zoned = ts.to_zoned(tz);
    Value::DateTime(Box::new(zoned))
}

/// DateTime with offset (tag 0x46/0x66): 3 fields -- epoch_secs, nanos, offset_seconds.
pub(crate) fn decode_datetime_offset(fields: &[BoltValue]) -> Value {
    let epoch_secs = match fields.first().and_then(bolt_int) {
        Some(s) => s,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'epoch_seconds' field in DateTimeOffset structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let nanos_adj = fields.get(1).and_then(bolt_int).unwrap_or(0);
    let offset_secs = match fields.get(2).and_then(bolt_int) {
        Some(o) => o,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'offset_seconds' field in DateTimeOffset structure, fields={fields:?}");
            return Value::Null;
        }
    };

    // When offset is exactly 0, use the named "UTC" timezone so the round-trip
    // preserves the IANA name.  This lets the Python pytz driver distinguish
    // named UTC from a bare +00:00 fixed offset (testkit 118/118).
    let tz = if offset_secs == 0 {
        jiff::tz::TimeZone::get("UTC").expect("UTC timezone must be available")
    } else {
        let offset_i32 = match i32::try_from(offset_secs) {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!("bolt_decode: offset_secs={offset_secs} overflows i32 in DateTimeOffset structure");
                return Value::Null;
            }
        };
        let offset = match jiff::tz::Offset::from_seconds(offset_i32) {
            Ok(o) => o,
            Err(_) => {
                tracing::warn!("bolt_decode: out-of-range offset_seconds={offset_secs} in DateTimeOffset structure");
                return Value::Null;
            }
        };
        jiff::tz::TimeZone::fixed(offset)
    };

    let nanos_i32 = match i32::try_from(nanos_adj) {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!(
                "bolt_decode: nanos_adj={nanos_adj} overflows i32 in DateTimeOffset structure"
            );
            return Value::Null;
        }
    };

    let ts = match jiff::Timestamp::new(epoch_secs, nanos_i32) {
        Ok(ts) => ts,
        Err(_) => {
            tracing::warn!("bolt_decode: out-of-range timestamp in DateTimeOffset, epoch_secs={epoch_secs}, nanos_adj={nanos_adj}");
            return Value::Null;
        }
    };

    let zoned = ts.to_zoned(tz);
    Value::DateTime(Box::new(zoned))
}

#[cfg(test)]
mod tests;
