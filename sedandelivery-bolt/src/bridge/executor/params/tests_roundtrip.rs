//! Full round-trip tests (bolt -> gql -> bolt).

use super::*;
use crate::bridge::types::scalar_value_to_bolt;

#[test]
fn roundtrip_date() {
    let bolt_in = BoltValue::Structure {
        tag: 0x44,
        fields: vec![BoltValue::Int(19737)],
    };
    let gql = bolt_value_to_gql(&bolt_in);
    let bolt_out = scalar_value_to_bolt(&gql);
    assert_eq!(bolt_in, bolt_out);
}

#[test]
fn roundtrip_local_time() {
    let nanos: i64 = 12 * 3_600_000_000_000 + 30 * 60_000_000_000;
    let bolt_in = BoltValue::Structure {
        tag: 0x74,
        fields: vec![BoltValue::Int(nanos)],
    };
    let gql = bolt_value_to_gql(&bolt_in);
    let bolt_out = scalar_value_to_bolt(&gql);
    assert_eq!(bolt_in, bolt_out);
}

#[test]
fn roundtrip_local_datetime() {
    let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60 + 45;
    let bolt_in = BoltValue::Structure {
        tag: 0x64,
        fields: vec![BoltValue::Int(epoch_secs), BoltValue::Int(123_456_789)],
    };
    let gql = bolt_value_to_gql(&bolt_in);
    let bolt_out = scalar_value_to_bolt(&gql);
    assert_eq!(bolt_in, bolt_out);
}

#[test]
fn roundtrip_datetime_zone_id() {
    let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60;
    let bolt_in = BoltValue::Structure {
        tag: 0x69,
        fields: vec![
            BoltValue::Int(epoch_secs),
            BoltValue::Int(0),
            BoltValue::String("UTC".to_string()),
        ],
    };
    let gql = bolt_value_to_gql(&bolt_in);
    let bolt_out = scalar_value_to_bolt(&gql);
    assert_eq!(bolt_in, bolt_out);
}

#[test]
fn roundtrip_datetime_offset() {
    let epoch_secs: i64 = 19737 * 86_400 + 7 * 3600 + 30 * 60;
    let bolt_in = BoltValue::Structure {
        tag: 0x49,
        fields: vec![
            BoltValue::Int(epoch_secs),
            BoltValue::Int(0),
            BoltValue::Int(10800),
        ],
    };
    let gql = bolt_value_to_gql(&bolt_in);
    let bolt_out = scalar_value_to_bolt(&gql);
    assert_eq!(bolt_in, bolt_out);
}

#[test]
fn roundtrip_duration() {
    let bolt_in = BoltValue::Structure {
        tag: 0x45,
        fields: vec![
            BoltValue::Int(14),
            BoltValue::Int(3),
            BoltValue::Int(5),
            BoltValue::Int(500_000_000),
        ],
    };
    let gql = bolt_value_to_gql(&bolt_in);
    let bolt_out = scalar_value_to_bolt(&gql);
    assert_eq!(bolt_in, bolt_out);
}

#[test]
fn roundtrip_point2d_wgs84() {
    let bolt_in = BoltValue::Structure {
        tag: 0x58,
        fields: vec![
            BoltValue::Int(4326),
            BoltValue::Float(-73.95),
            BoltValue::Float(40.78),
        ],
    };
    let gql = bolt_value_to_gql(&bolt_in);
    let bolt_out = scalar_value_to_bolt(&gql);
    assert_eq!(bolt_in, bolt_out);
}

#[test]
fn roundtrip_point2d_cartesian() {
    let bolt_in = BoltValue::Structure {
        tag: 0x58,
        fields: vec![
            BoltValue::Int(7203),
            BoltValue::Float(1.5),
            BoltValue::Float(2.5),
        ],
    };
    let gql = bolt_value_to_gql(&bolt_in);
    let bolt_out = scalar_value_to_bolt(&gql);
    assert_eq!(bolt_in, bolt_out);
}
