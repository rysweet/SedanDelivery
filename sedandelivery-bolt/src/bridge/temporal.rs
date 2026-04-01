//! Temporal Bolt structure decoders.
//!
//! Decodes Bolt Date, LocalTime, Time, LocalDateTime, DateTimeZoneId,
//! DateTimeOffset, and Duration structures into cypher-core `Value` types.

use cypher_core::types::value::CypherDuration;
use cypher_core::types::Value;

use crate::packstream::value::BoltValue;

/// Extract an i64 from a BoltValue, returning None if it is not an Int.
pub(crate) fn bolt_int(v: &BoltValue) -> Option<i64> {
    match v {
        BoltValue::Int(i) => Some(*i),
        _ => None,
    }
}

/// Date (tag 0x44): 1 field -- days since 1970-01-01.
pub(crate) fn decode_date(fields: &[BoltValue]) -> Value {
    let days = match fields.first().and_then(bolt_int) {
        Some(d) => d,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'days' field in Date structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let epoch = jiff::civil::Date::new(1970, 1, 1).expect("valid epoch");
    match epoch.checked_add(jiff::Span::new().days(days)) {
        Ok(d) => Value::Date(d),
        Err(_) => {
            tracing::warn!("bolt_decode: date overflow, days={days} out of representable range");
            Value::Null
        }
    }
}

/// LocalTime (tag 0x74): 1 field -- nanoseconds since midnight.
pub(crate) fn decode_local_time(fields: &[BoltValue]) -> Value {
    let nanos = match fields.first().and_then(bolt_int) {
        Some(n) => n,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'nanos' field in LocalTime structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let h = (nanos / 3_600_000_000_000) as i8;
    let rem = nanos % 3_600_000_000_000;
    let m = (rem / 60_000_000_000) as i8;
    let rem = rem % 60_000_000_000;
    let s = (rem / 1_000_000_000) as i8;
    let ns = (rem % 1_000_000_000) as i32;
    match jiff::civil::Time::new(h, m, s, ns) {
        Ok(t) => Value::LocalTime(t),
        Err(_) => {
            tracing::warn!("bolt_decode: out-of-range LocalTime, nanos={nanos}");
            Value::Null
        }
    }
}

/// Time with UTC offset (tag 0x54): 2 fields -- nanoseconds since midnight, offset_seconds.
/// Returns a Value::Time (time + offset) representing a zoned time with no date component.
pub(crate) fn decode_time_with_offset(fields: &[BoltValue]) -> Value {
    let nanos = match fields.first().and_then(bolt_int) {
        Some(n) => n,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'nanos' field in Time structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let offset_secs = match fields.get(1).and_then(bolt_int) {
        Some(o) => o,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'offset_seconds' field in Time structure, fields={fields:?}");
            return Value::Null;
        }
    };

    let h = (nanos / 3_600_000_000_000) as i8;
    let rem = nanos % 3_600_000_000_000;
    let m = (rem / 60_000_000_000) as i8;
    let rem = rem % 60_000_000_000;
    let s = (rem / 1_000_000_000) as i8;
    let ns = (rem % 1_000_000_000) as i32;

    let offset_i32 = match i32::try_from(offset_secs) {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!(
                "bolt_decode: offset_secs={offset_secs} overflows i32 in Time structure"
            );
            return Value::Null;
        }
    };

    match jiff::civil::Time::new(h, m, s, ns) {
        Ok(t) => Value::Time(t, offset_i32),
        Err(_) => {
            tracing::warn!(
                "bolt_decode: out-of-range Time, nanos={nanos}, offset_secs={offset_secs}"
            );
            Value::Null
        }
    }
}

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

/// Duration (tag 0x45): 4 fields -- months, days, seconds, nanoseconds.
/// All four fields are required; a missing field is an error, not silently zero.
pub(crate) fn decode_duration(fields: &[BoltValue]) -> Value {
    let months = match fields.first().and_then(bolt_int) {
        Some(v) => v,
        None => {
            tracing::error!("bolt_decode: missing or non-integer 'months' field in Duration structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let days = match fields.get(1).and_then(bolt_int) {
        Some(v) => v,
        None => {
            tracing::error!("bolt_decode: missing or non-integer 'days' field in Duration structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let seconds = match fields.get(2).and_then(bolt_int) {
        Some(v) => v,
        None => {
            tracing::error!("bolt_decode: missing or non-integer 'seconds' field in Duration structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let nanos = match fields.get(3).and_then(bolt_int) {
        Some(v) => v,
        None => {
            tracing::error!("bolt_decode: missing or non-integer 'nanoseconds' field in Duration structure, fields={fields:?}");
            return Value::Null;
        }
    };

    Value::Duration(CypherDuration {
        months,
        days,
        nanos: (seconds as i128) * 1_000_000_000 + (nanos as i128),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Date tests ──────────────────────────────────────────────────────────

    #[test]
    fn decode_date_from_bolt_structure() {
        let val = decode_date(&[BoltValue::Int(19737)]);
        let expected = jiff::civil::Date::new(2024, 1, 15).unwrap();
        assert_eq!(val, Value::Date(expected));
    }

    #[test]
    fn decode_date_epoch() {
        let val = decode_date(&[BoltValue::Int(0)]);
        let expected = jiff::civil::Date::new(1970, 1, 1).unwrap();
        assert_eq!(val, Value::Date(expected));
    }

    #[test]
    fn decode_date_missing_field_returns_null() {
        assert_eq!(decode_date(&[]), Value::Null);
    }

    #[test]
    fn decode_date_non_integer_field_returns_null() {
        assert_eq!(
            decode_date(&[BoltValue::String("not-a-number".into())]),
            Value::Null
        );
    }

    #[test]
    fn decode_date_overflow_returns_null() {
        assert_eq!(decode_date(&[BoltValue::Int(7_304_484)]), Value::Null);
    }

    // ── LocalTime tests ─────────────────────────────────────────────────────

    #[test]
    fn decode_local_time_from_bolt_structure() {
        let val = decode_local_time(&[BoltValue::Int(45_000_000_000_000)]);
        let expected = jiff::civil::Time::new(12, 30, 0, 0).unwrap();
        assert_eq!(val, Value::LocalTime(expected));
    }

    #[test]
    fn decode_local_time_with_nanos() {
        let nanos: i64 =
            10 * 3_600_000_000_000 + 20 * 60_000_000_000 + 30 * 1_000_000_000 + 123_456_789;
        let val = decode_local_time(&[BoltValue::Int(nanos)]);
        let expected = jiff::civil::Time::new(10, 20, 30, 123_456_789).unwrap();
        assert_eq!(val, Value::LocalTime(expected));
    }

    #[test]
    fn decode_local_time_missing_field_returns_null() {
        assert_eq!(decode_local_time(&[]), Value::Null);
    }

    #[test]
    fn decode_local_time_non_integer_returns_null() {
        assert_eq!(decode_local_time(&[BoltValue::Bool(true)]), Value::Null);
    }

    #[test]
    fn decode_local_time_out_of_range_returns_null() {
        let nanos_25h = 25 * 3_600_000_000_000_i64;
        assert_eq!(decode_local_time(&[BoltValue::Int(nanos_25h)]), Value::Null);
    }

    // ── Time with offset tests ──────────────────────────────────────────────

    #[test]
    fn decode_time_with_offset_missing_nanos_returns_null() {
        assert_eq!(decode_time_with_offset(&[]), Value::Null);
    }

    #[test]
    fn decode_time_with_offset_missing_offset_returns_null() {
        assert_eq!(decode_time_with_offset(&[BoltValue::Int(0)]), Value::Null);
    }

    #[test]
    fn decode_time_with_offset_non_integer_offset_returns_null() {
        assert_eq!(
            decode_time_with_offset(&[BoltValue::Int(0), BoltValue::String("bad".into())]),
            Value::Null
        );
    }

    #[test]
    fn decode_time_with_offset_out_of_range_returns_null() {
        let nanos_25h = 25 * 3_600_000_000_000_i64;
        assert_eq!(
            decode_time_with_offset(&[BoltValue::Int(nanos_25h), BoltValue::Int(0)]),
            Value::Null
        );
    }

    #[test]
    fn decode_time_offset_i32_overflow_returns_null() {
        let nanos_noon = 12 * 3_600_000_000_000_i64;
        assert_eq!(
            decode_time_with_offset(&[
                BoltValue::Int(nanos_noon),
                BoltValue::Int(i64::from(i32::MAX) + 1)
            ]),
            Value::Null
        );
    }

    // ── LocalDateTime tests ─────────────────────────────────────────────────

    #[test]
    fn decode_local_datetime_from_bolt_structure() {
        let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60 + 45;
        let val = decode_local_datetime(&[BoltValue::Int(epoch_secs), BoltValue::Int(123_456_789)]);
        let expected = jiff::civil::DateTime::new(2024, 1, 15, 10, 30, 45, 123_456_789).unwrap();
        assert_eq!(val, Value::LocalDateTime(expected));
    }

    #[test]
    fn decode_local_datetime_missing_epoch_returns_null() {
        assert_eq!(decode_local_datetime(&[]), Value::Null);
    }

    #[test]
    fn decode_local_datetime_non_integer_returns_null() {
        assert_eq!(decode_local_datetime(&[BoltValue::Float(1.0)]), Value::Null);
    }

    #[test]
    fn decode_local_datetime_overflow_returns_null() {
        let huge_epoch = 7_304_484_i64 * 86_400;
        assert_eq!(
            decode_local_datetime(&[BoltValue::Int(huge_epoch), BoltValue::Int(0)]),
            Value::Null
        );
    }

    #[test]
    fn decode_local_datetime_nanos_i32_overflow_returns_null() {
        assert_eq!(
            decode_local_datetime(&[BoltValue::Int(0), BoltValue::Int(i64::from(i32::MAX) + 1)]),
            Value::Null
        );
    }

    // ── DateTimeZoneId tests ────────────────────────────────────────────────

    #[test]
    fn decode_datetime_zone_id_tag_0x69() {
        let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60;
        let val = decode_datetime_zone_id(&[
            BoltValue::Int(epoch_secs),
            BoltValue::Int(0),
            BoltValue::String("UTC".to_string()),
        ]);
        if let Value::DateTime(zoned) = &val {
            assert_eq!(zoned.time_zone().iana_name(), Some("UTC"));
            assert_eq!(zoned.timestamp().as_second(), epoch_secs);
        } else {
            panic!("expected Value::DateTime, got {:?}", val);
        }
    }

    #[test]
    fn decode_datetime_zone_id_tag_0x66() {
        let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60;
        let val = decode_datetime_zone_id(&[
            BoltValue::Int(epoch_secs),
            BoltValue::Int(0),
            BoltValue::String("America/New_York".to_string()),
        ]);
        if let Value::DateTime(zoned) = &val {
            assert!(zoned.time_zone().iana_name().is_some());
        } else {
            panic!("expected Value::DateTime, got {:?}", val);
        }
    }

    #[test]
    fn decode_datetime_zone_id_missing_epoch_returns_null() {
        assert_eq!(decode_datetime_zone_id(&[]), Value::Null);
    }

    #[test]
    fn decode_datetime_zone_id_missing_tz_returns_null() {
        assert_eq!(
            decode_datetime_zone_id(&[BoltValue::Int(0), BoltValue::Int(0)]),
            Value::Null
        );
    }

    #[test]
    fn decode_datetime_zone_id_non_string_tz_returns_null() {
        assert_eq!(
            decode_datetime_zone_id(&[BoltValue::Int(0), BoltValue::Int(0), BoltValue::Int(999)]),
            Value::Null
        );
    }

    #[test]
    fn decode_datetime_zone_id_invalid_tz_returns_null() {
        assert_eq!(
            decode_datetime_zone_id(&[
                BoltValue::Int(0),
                BoltValue::Int(0),
                BoltValue::String("Not/A/Real/Timezone".into())
            ]),
            Value::Null
        );
    }

    #[test]
    fn decode_datetime_zone_id_overflow_returns_null() {
        assert_eq!(
            decode_datetime_zone_id(&[
                BoltValue::Int(i64::MAX),
                BoltValue::Int(0),
                BoltValue::String("UTC".into())
            ]),
            Value::Null
        );
    }

    #[test]
    fn decode_datetime_zone_id_nanos_i32_overflow_returns_null() {
        assert_eq!(
            decode_datetime_zone_id(&[
                BoltValue::Int(0),
                BoltValue::Int(i64::from(i32::MAX) + 1),
                BoltValue::String("UTC".into())
            ]),
            Value::Null
        );
    }

    // ── DateTimeOffset tests ────────────────────────────────────────────────

    #[test]
    fn decode_datetime_offset_from_bolt_structure() {
        let epoch_secs: i64 = 19737 * 86_400 + 7 * 3600 + 30 * 60;
        let val = decode_datetime_offset(&[
            BoltValue::Int(epoch_secs),
            BoltValue::Int(0),
            BoltValue::Int(10800),
        ]);
        if let Value::DateTime(zoned) = &val {
            assert_eq!(zoned.offset().seconds(), 10800);
            assert_eq!(zoned.timestamp().as_second(), epoch_secs);
        } else {
            panic!("expected Value::DateTime, got {:?}", val);
        }
    }

    #[test]
    fn decode_datetime_offset_missing_epoch_returns_null() {
        assert_eq!(decode_datetime_offset(&[]), Value::Null);
    }

    #[test]
    fn decode_datetime_offset_missing_offset_returns_null() {
        assert_eq!(
            decode_datetime_offset(&[BoltValue::Int(0), BoltValue::Int(0)]),
            Value::Null
        );
    }

    #[test]
    fn decode_datetime_offset_non_integer_offset_returns_null() {
        assert_eq!(
            decode_datetime_offset(&[
                BoltValue::Int(0),
                BoltValue::Int(0),
                BoltValue::String("bad".into())
            ]),
            Value::Null
        );
    }

    #[test]
    fn decode_datetime_offset_out_of_range_offset_returns_null() {
        assert_eq!(
            decode_datetime_offset(&[
                BoltValue::Int(0),
                BoltValue::Int(0),
                BoltValue::Int(999_999)
            ]),
            Value::Null
        );
    }

    #[test]
    fn decode_datetime_offset_overflow_timestamp_returns_null() {
        assert_eq!(
            decode_datetime_offset(&[
                BoltValue::Int(i64::MAX),
                BoltValue::Int(0),
                BoltValue::Int(3600)
            ]),
            Value::Null
        );
    }

    #[test]
    fn decode_datetime_offset_offset_i32_overflow_returns_null() {
        assert_eq!(
            decode_datetime_offset(&[
                BoltValue::Int(0),
                BoltValue::Int(0),
                BoltValue::Int(i64::from(i32::MAX) + 1)
            ]),
            Value::Null
        );
    }

    #[test]
    fn decode_datetime_offset_nanos_i32_overflow_returns_null() {
        assert_eq!(
            decode_datetime_offset(&[
                BoltValue::Int(0),
                BoltValue::Int(i64::from(i32::MAX) + 1),
                BoltValue::Int(3600)
            ]),
            Value::Null
        );
    }

    // ── Duration tests ──────────────────────────────────────────────────────

    #[test]
    fn decode_duration_from_bolt_structure() {
        let val = decode_duration(&[
            BoltValue::Int(14),
            BoltValue::Int(3),
            BoltValue::Int(100),
            BoltValue::Int(0),
        ]);
        let expected = CypherDuration {
            months: 14,
            days: 3,
            nanos: 100_000_000_000,
        };
        assert_eq!(val, Value::Duration(expected));
    }

    #[test]
    fn decode_duration_with_sub_second_nanos() {
        let val = decode_duration(&[
            BoltValue::Int(0),
            BoltValue::Int(0),
            BoltValue::Int(5),
            BoltValue::Int(500_000_000),
        ]);
        let expected = CypherDuration {
            months: 0,
            days: 0,
            nanos: 5_500_000_000,
        };
        assert_eq!(val, Value::Duration(expected));
    }

    #[test]
    fn decode_duration_missing_fields_returns_null() {
        assert_eq!(decode_duration(&[]), Value::Null);
    }

    #[test]
    fn decode_duration_partial_fields_returns_null() {
        assert_eq!(decode_duration(&[BoltValue::Int(6)]), Value::Null);
    }
}
