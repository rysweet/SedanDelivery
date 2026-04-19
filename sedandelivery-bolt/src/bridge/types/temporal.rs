//! Temporal Cypher → Bolt encoders (Date, Time, DateTime, Duration).

use crate::packstream::value::BoltValue;
use crate::protocol::tags;

/// Date: 1 field (days since 1970-01-01 epoch).
pub(super) fn date_to_bolt(d: &jiff::civil::Date) -> BoltValue {
    let epoch = jiff::civil::Date::new(1970, 1, 1).expect("valid epoch date");
    let days = match d.since(epoch) {
        Ok(span) => span.get_days() as i64,
        Err(_) => {
            tracing::warn!("bolt_encode: date diff overflow for date={d}, returning Null");
            return BoltValue::Null;
        }
    };
    BoltValue::Structure {
        tag: tags::TAG_DATE,
        fields: vec![BoltValue::Int(days)],
    }
}

/// LocalTime: 1 field (nanoseconds since midnight).
pub(super) fn local_time_to_bolt(t: &jiff::civil::Time) -> BoltValue {
    // Compute total nanoseconds since midnight from individual components.
    let nanos: i64 = (t.hour() as i64) * 3_600_000_000_000
        + (t.minute() as i64) * 60_000_000_000
        + (t.second() as i64) * 1_000_000_000
        + (t.subsec_nanosecond() as i64);
    BoltValue::Structure {
        tag: tags::TAG_LOCAL_TIME,
        fields: vec![BoltValue::Int(nanos)],
    }
}

/// LocalDateTime: 2 fields (epoch_seconds, nanosecond_adjustment).
pub(super) fn local_datetime_to_bolt(dt: &jiff::civil::DateTime) -> BoltValue {
    let epoch_date = jiff::civil::Date::new(1970, 1, 1).expect("valid epoch date");
    let days = match dt.date().since(epoch_date) {
        Ok(span) => span.get_days() as i64,
        Err(_) => {
            tracing::warn!("bolt_encode: date diff overflow for datetime={dt}, returning Null");
            return BoltValue::Null;
        }
    };
    // Compute epoch seconds from days + time-of-day components.
    let total_seconds: i64 = days * 86_400
        + (dt.hour() as i64) * 3_600
        + (dt.minute() as i64) * 60
        + (dt.second() as i64);
    // Sub-second nanosecond adjustment: use the datetime's own subsec_nanosecond()
    // directly rather than span decomposition, because jiff's Span splits sub-second
    // precision across milliseconds, microseconds, and nanoseconds fields.
    let nanos_adj = dt.subsec_nanosecond() as i64;
    BoltValue::Structure {
        tag: tags::TAG_LOCAL_DATE_TIME,
        fields: vec![BoltValue::Int(total_seconds), BoltValue::Int(nanos_adj)],
    }
}

/// Zoned time: Bolt Time encoding.
///
/// 2 fields: nanoseconds_since_midnight, offset_seconds.
/// Used for `Value::Time` which represents a time-of-day with UTC offset
/// but no date component.
pub(super) fn zoned_time_to_bolt(t: &jiff::civil::Time, offset_secs: i32) -> BoltValue {
    let nanos_since_midnight = (t.hour() as i64) * 3_600_000_000_000
        + (t.minute() as i64) * 60_000_000_000
        + (t.second() as i64) * 1_000_000_000
        + (t.subsec_nanosecond() as i64);
    BoltValue::Structure {
        tag: tags::TAG_TIME,
        fields: vec![
            BoltValue::Int(nanos_since_midnight),
            BoltValue::Int(offset_secs as i64),
        ],
    }
}

/// DateTime (zoned): Bolt UTC-mode encoding (patch_bolt: utc).
///
/// When an IANA timezone name is available, uses `TAG_DATETIME_ZONE_ID_UTC`.
/// When only a fixed UTC offset is available, uses `TAG_DATETIME_ZONE_OFFSET_UTC`.
///
/// Since we negotiate patch_bolt: \["utc"\], we always use UTC-mode tags.
pub(super) fn datetime_to_bolt(zoned: &jiff::Zoned) -> BoltValue {
    let ts = zoned.timestamp();
    // jiff's as_second() truncates toward zero, which for negative timestamps
    // can produce a negative subsec_nanosecond(). Bolt requires nanoseconds
    // to be 0..999999999, so normalize: floor-divide the seconds and ensure
    // nanoseconds are non-negative.
    let raw_secs = ts.as_second();
    let raw_nanos = ts.subsec_nanosecond();
    let (epoch_seconds, nanos_adj) = if raw_nanos < 0 {
        (raw_secs - 1, (raw_nanos + 1_000_000_000) as i64)
    } else {
        (raw_secs, raw_nanos as i64)
    };

    if let Some(tz_name) = zoned.time_zone().iana_name() {
        // The IANA "Factory" timezone is valid in jiff's bundled tz database, but
        // pytz (used by the Neo4j Python driver) doesn't recognize it, causing
        // hydration failures. Encode it as a fixed-offset DateTime instead.
        if tz_name == "Factory" {
            let offset_seconds = zoned.offset().seconds() as i64;
            return BoltValue::Structure {
                tag: tags::TAG_DATETIME_ZONE_OFFSET_UTC,
                fields: vec![
                    BoltValue::Int(epoch_seconds),
                    BoltValue::Int(nanos_adj),
                    BoltValue::Int(offset_seconds),
                ],
            };
        }
        // IANA timezone name available: use DateTimeZoneId (UTC mode).
        BoltValue::Structure {
            tag: tags::TAG_DATETIME_ZONE_ID_UTC,
            fields: vec![
                BoltValue::Int(epoch_seconds),
                BoltValue::Int(nanos_adj),
                BoltValue::String(tz_name.to_string()),
            ],
        }
    } else {
        // Fixed offset only: use DateTimeZoneOffset (UTC mode).
        let offset_seconds = zoned.offset().seconds() as i64;
        BoltValue::Structure {
            tag: tags::TAG_DATETIME_ZONE_OFFSET_UTC,
            fields: vec![
                BoltValue::Int(epoch_seconds),
                BoltValue::Int(nanos_adj),
                BoltValue::Int(offset_seconds),
            ],
        }
    }
}

/// Duration: 4 fields (months, days, seconds, nanoseconds).
pub(super) fn duration_to_bolt(dur: &cypher_core::types::CypherDuration) -> BoltValue {
    // CypherDuration stores nanos as i128. Split into seconds + sub-second nanos.
    let total_nanos = dur.nanos;
    let mut seconds = (total_nanos / 1_000_000_000) as i64;
    let mut nanos = (total_nanos % 1_000_000_000) as i32;
    // Normalize: Bolt spec requires nanoseconds in 0..999_999_999.
    // Rust's `%` on negative values produces negative remainders.
    if nanos < 0 {
        seconds -= 1;
        nanos += 1_000_000_000;
    }
    let nanos = nanos as i64;
    BoltValue::Structure {
        tag: tags::TAG_DURATION,
        fields: vec![
            BoltValue::Int(dur.months),
            BoltValue::Int(dur.days),
            BoltValue::Int(seconds),
            BoltValue::Int(nanos),
        ],
    }
}
