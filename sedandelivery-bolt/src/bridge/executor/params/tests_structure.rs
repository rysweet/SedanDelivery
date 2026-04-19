//! Structure decoding tests (bolt -> gql via `bolt_value_to_gql`).

use super::*;
use cypher_core::types::value::{Crs, CypherDuration, GeoPoint};

#[test]
fn decode_date_from_bolt_structure() {
    let bv = BoltValue::Structure {
        tag: 0x44,
        fields: vec![BoltValue::Int(19737)],
    };
    let val = bolt_value_to_gql(&bv);
    let expected = jiff::civil::Date::new(2024, 1, 15).unwrap();
    assert_eq!(val, Value::Date(expected));
}

#[test]
fn decode_date_epoch() {
    let bv = BoltValue::Structure {
        tag: 0x44,
        fields: vec![BoltValue::Int(0)],
    };
    let val = bolt_value_to_gql(&bv);
    let expected = jiff::civil::Date::new(1970, 1, 1).unwrap();
    assert_eq!(val, Value::Date(expected));
}

#[test]
fn decode_local_time_from_bolt_structure() {
    let bv = BoltValue::Structure {
        tag: 0x74,
        fields: vec![BoltValue::Int(45_000_000_000_000)],
    };
    let val = bolt_value_to_gql(&bv);
    let expected = jiff::civil::Time::new(12, 30, 0, 0).unwrap();
    assert_eq!(val, Value::LocalTime(expected));
}

#[test]
fn decode_local_time_with_nanos() {
    let nanos: i64 =
        10 * 3_600_000_000_000 + 20 * 60_000_000_000 + 30 * 1_000_000_000 + 123_456_789;
    let bv = BoltValue::Structure {
        tag: 0x74,
        fields: vec![BoltValue::Int(nanos)],
    };
    let val = bolt_value_to_gql(&bv);
    let expected = jiff::civil::Time::new(10, 20, 30, 123_456_789).unwrap();
    assert_eq!(val, Value::LocalTime(expected));
}

#[test]
fn decode_local_datetime_from_bolt_structure() {
    let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60 + 45;
    let bv = BoltValue::Structure {
        tag: 0x64,
        fields: vec![BoltValue::Int(epoch_secs), BoltValue::Int(123_456_789)],
    };
    let val = bolt_value_to_gql(&bv);
    let expected = jiff::civil::DateTime::new(2024, 1, 15, 10, 30, 45, 123_456_789).unwrap();
    assert_eq!(val, Value::LocalDateTime(expected));
}

#[test]
fn decode_datetime_zone_id_tag_0x69() {
    let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60;
    let bv = BoltValue::Structure {
        tag: 0x69,
        fields: vec![
            BoltValue::Int(epoch_secs),
            BoltValue::Int(0),
            BoltValue::String("UTC".to_string()),
        ],
    };
    let val = bolt_value_to_gql(&bv);
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
    let bv = BoltValue::Structure {
        tag: 0x66,
        fields: vec![
            BoltValue::Int(epoch_secs),
            BoltValue::Int(0),
            BoltValue::String("America/New_York".to_string()),
        ],
    };
    let val = bolt_value_to_gql(&bv);
    if let Value::DateTime(zoned) = &val {
        assert!(zoned.time_zone().iana_name().is_some());
    } else {
        panic!("expected Value::DateTime, got {:?}", val);
    }
}

#[test]
fn decode_datetime_offset_from_bolt_structure() {
    let epoch_secs: i64 = 19737 * 86_400 + 7 * 3600 + 30 * 60;
    let bv = BoltValue::Structure {
        tag: 0x46,
        fields: vec![
            BoltValue::Int(epoch_secs),
            BoltValue::Int(0),
            BoltValue::Int(10800),
        ],
    };
    let val = bolt_value_to_gql(&bv);
    if let Value::DateTime(zoned) = &val {
        assert_eq!(zoned.offset().seconds(), 10800);
        assert_eq!(zoned.timestamp().as_second(), epoch_secs);
    } else {
        panic!("expected Value::DateTime, got {:?}", val);
    }
}

#[test]
fn decode_duration_from_bolt_structure() {
    let bv = BoltValue::Structure {
        tag: 0x45,
        fields: vec![
            BoltValue::Int(14),
            BoltValue::Int(3),
            BoltValue::Int(100),
            BoltValue::Int(0),
        ],
    };
    let val = bolt_value_to_gql(&bv);
    let expected = CypherDuration {
        months: 14,
        days: 3,
        nanos: 100_000_000_000,
    };
    assert_eq!(val, Value::Duration(expected));
}

#[test]
fn decode_duration_with_sub_second_nanos() {
    let bv = BoltValue::Structure {
        tag: 0x45,
        fields: vec![
            BoltValue::Int(0),
            BoltValue::Int(0),
            BoltValue::Int(5),
            BoltValue::Int(500_000_000),
        ],
    };
    let val = bolt_value_to_gql(&bv);
    let expected = CypherDuration {
        months: 0,
        days: 0,
        nanos: 5_500_000_000,
    };
    assert_eq!(val, Value::Duration(expected));
}

#[test]
fn decode_point2d_wgs84_from_bolt_structure() {
    let bv = BoltValue::Structure {
        tag: 0x58,
        fields: vec![
            BoltValue::Int(4326),
            BoltValue::Float(-73.95),
            BoltValue::Float(40.78),
        ],
    };
    let val = bolt_value_to_gql(&bv);
    let expected = GeoPoint {
        x: -73.95,
        y: 40.78,
        z: None,
        crs: Crs::Wgs84,
    };
    assert_eq!(val, Value::Point(expected));
}

#[test]
fn decode_point2d_cartesian_from_bolt_structure() {
    let bv = BoltValue::Structure {
        tag: 0x58,
        fields: vec![
            BoltValue::Int(7203),
            BoltValue::Float(1.5),
            BoltValue::Float(2.5),
        ],
    };
    let val = bolt_value_to_gql(&bv);
    let expected = GeoPoint {
        x: 1.5,
        y: 2.5,
        z: None,
        crs: Crs::Cartesian,
    };
    assert_eq!(val, Value::Point(expected));
}

#[test]
fn decode_point3d_from_bolt_structure() {
    let bv = BoltValue::Structure {
        tag: 0x59,
        fields: vec![
            BoltValue::Int(4979),
            BoltValue::Float(1.0),
            BoltValue::Float(2.0),
            BoltValue::Float(3.0),
        ],
    };
    let val = bolt_value_to_gql(&bv);
    let expected = GeoPoint {
        x: 1.0,
        y: 2.0,
        z: Some(3.0),
        crs: Crs::Wgs843d,
    };
    assert_eq!(val, Value::Point(expected));
}

#[test]
fn decode_unknown_structure_returns_null() {
    let bv = BoltValue::Structure {
        tag: 0xFF,
        fields: vec![BoltValue::Int(1)],
    };
    let val = bolt_value_to_gql(&bv);
    assert_eq!(val, Value::Null);
}

#[test]
fn bolt_structure_unknown_tag_returns_null() {
    let val = bolt_structure_to_gql(0xFF, &[BoltValue::Int(1)]);
    assert_eq!(val, Value::Null);
}
