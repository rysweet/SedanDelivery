//! Convert Bolt-protocol parameters and structures into cypher-core values.

use std::collections::HashMap;

use cypher_core::types::Value;
use indexmap::IndexMap;

use crate::bridge::spatial::{decode_point2d, decode_point3d};
use crate::bridge::temporal::{
    decode_date, decode_datetime_offset, decode_datetime_zone_id, decode_duration,
    decode_local_datetime, decode_local_time, decode_time_with_offset,
};
use crate::packstream::value::{BoltMap, BoltValue};
use crate::protocol::tags;

/// Convert a `BoltMap` (from the Bolt RUN message's parameters field) into
/// a `HashMap<String, Value>` suitable for the cypher-core query executor.
pub fn bolt_map_to_value_map(bolt_map: &BoltMap) -> HashMap<String, Value> {
    bolt_map
        .iter()
        .map(|(k, v)| (k.clone(), bolt_value_to_gql(v)))
        .collect()
}

/// Convert a single BoltValue to a cypher-core Value.
fn bolt_value_to_gql(bv: &BoltValue) -> Value {
    match bv {
        BoltValue::Null => Value::Null,
        BoltValue::Bool(b) => Value::Bool(*b),
        BoltValue::Int(i) => Value::Int(*i),
        BoltValue::Float(f) => Value::Float(*f),
        BoltValue::String(s) => Value::Str(s.as_str().into()),
        BoltValue::Bytes(b) => Value::Bytes(b.clone()),
        BoltValue::List(items) => Value::List(items.iter().map(bolt_value_to_gql).collect()),
        BoltValue::Map(entries) => {
            let mut map = IndexMap::new();
            for (k, v) in entries {
                map.insert(Box::from(k.as_str()), bolt_value_to_gql(v));
            }
            Value::Map(map)
        }
        BoltValue::Structure { tag, fields } => bolt_structure_to_gql(*tag, fields),
    }
}

/// Decode a Bolt Structure (temporal or spatial) into a cypher-core Value.
/// Unrecognized structure tags fall back to Value::Null.
fn bolt_structure_to_gql(tag: u8, fields: &[BoltValue]) -> Value {
    match tag {
        tags::TAG_DATE => decode_date(fields),
        tags::TAG_TIME => decode_time_with_offset(fields),
        tags::TAG_LOCAL_TIME => decode_local_time(fields),
        tags::TAG_LOCAL_DATE_TIME => decode_local_datetime(fields),
        tags::TAG_DATETIME_ZONE_ID_UTC | tags::TAG_DATETIME_ZONE_ID_WALL => {
            decode_datetime_zone_id(fields)
        }
        tags::TAG_DATETIME_ZONE_OFFSET_UTC | tags::TAG_DATETIME_ZONE_OFFSET_WALL => {
            decode_datetime_offset(fields)
        }
        tags::TAG_DURATION => decode_duration(fields),
        tags::TAG_POINT_2D => decode_point2d(fields),
        tags::TAG_POINT_3D => decode_point3d(fields),
        _ => {
            tracing::error!(tag = format_args!("0x{tag:02X}"), "bolt_decode: unrecognized structure tag; this likely indicates a protocol version mismatch or corrupt data");
            Value::Null
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::executor::{execute_query, result_to_records};
    use crate::bridge::types::scalar_value_to_bolt;
    use cypher_core::storage::InMemoryGraphStore;
    use cypher_core::types::value::{Crs, CypherDuration, GeoPoint};

    #[test]
    fn bolt_map_to_value_map_basic() {
        let bolt_map: BoltMap = vec![
            ("name".to_string(), BoltValue::String("Alice".to_string())),
            ("age".to_string(), BoltValue::Int(30)),
            ("active".to_string(), BoltValue::Bool(true)),
        ];

        let value_map = bolt_map_to_value_map(&bolt_map);
        assert_eq!(value_map.len(), 3);
        assert_eq!(value_map["name"], Value::Str("Alice".into()));
        assert_eq!(value_map["age"], Value::Int(30));
        assert_eq!(value_map["active"], Value::Bool(true));
    }

    #[test]
    fn bolt_map_to_value_map_nested() {
        let inner_map: BoltMap = vec![("x".to_string(), BoltValue::Int(1))];
        let bolt_map: BoltMap = vec![
            (
                "list".to_string(),
                BoltValue::List(vec![BoltValue::Int(1), BoltValue::Int(2)]),
            ),
            ("map".to_string(), BoltValue::Map(inner_map)),
            ("null".to_string(), BoltValue::Null),
        ];

        let value_map = bolt_map_to_value_map(&bolt_map);
        assert_eq!(value_map.len(), 3);
        if let Value::List(items) = &value_map["list"] {
            assert_eq!(items.len(), 2);
        } else {
            panic!("expected List");
        }
        if let Value::Map(m) = &value_map["map"] {
            assert_eq!(m.len(), 1);
        } else {
            panic!("expected Map");
        }
        assert_eq!(value_map["null"], Value::Null);
    }

    #[test]
    fn bolt_map_to_value_map_empty() {
        let bolt_map: BoltMap = vec![];
        let value_map = bolt_map_to_value_map(&bolt_map);
        assert!(value_map.is_empty());
    }

    #[test]
    fn bolt_bytes_converts_to_bytes() {
        let bolt_map: BoltMap = vec![("data".to_string(), BoltValue::Bytes(vec![0x01, 0x02]))];
        let value_map = bolt_map_to_value_map(&bolt_map);
        assert_eq!(value_map["data"], Value::Bytes(vec![0x01, 0x02]));
    }

    // ── Structure decoding integration tests (bolt -> gql via bolt_value_to_gql) ──

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

    // ── Full round-trip tests (bolt -> gql -> bolt) ─────────────────────────

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

    // ── End-to-end round-trip via query params ──────────────────────────────

    #[test]
    fn roundtrip_date_via_query_params() {
        let store = InMemoryGraphStore::new();
        let bolt_params: BoltMap = vec![(
            "d".to_string(),
            BoltValue::Structure {
                tag: 0x44,
                fields: vec![BoltValue::Int(19737)],
            },
        )];
        let params = bolt_map_to_value_map(&bolt_params);
        let result = execute_query(&store, "default", "RETURN $d AS d", params, None).unwrap();
        let (records, _, _, _) = result_to_records(&result, &store, "default");
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0][0],
            BoltValue::Structure {
                tag: 0x44,
                fields: vec![BoltValue::Int(19737)]
            }
        );
    }

    #[test]
    fn roundtrip_duration_via_query_params() {
        let store = InMemoryGraphStore::new();
        let bolt_params: BoltMap = vec![(
            "d".to_string(),
            BoltValue::Structure {
                tag: 0x45,
                fields: vec![
                    BoltValue::Int(14),
                    BoltValue::Int(3),
                    BoltValue::Int(100),
                    BoltValue::Int(0),
                ],
            },
        )];
        let params = bolt_map_to_value_map(&bolt_params);
        let result = execute_query(&store, "default", "RETURN $d AS d", params, None).unwrap();
        let (records, _, _, _) = result_to_records(&result, &store, "default");
        assert_eq!(
            records[0][0],
            BoltValue::Structure {
                tag: 0x45,
                fields: vec![
                    BoltValue::Int(14),
                    BoltValue::Int(3),
                    BoltValue::Int(100),
                    BoltValue::Int(0)
                ]
            }
        );
    }

    #[test]
    fn roundtrip_point_via_query_params() {
        let store = InMemoryGraphStore::new();
        let bolt_params: BoltMap = vec![(
            "p".to_string(),
            BoltValue::Structure {
                tag: 0x58,
                fields: vec![
                    BoltValue::Int(7203),
                    BoltValue::Float(1.5),
                    BoltValue::Float(2.5),
                ],
            },
        )];
        let params = bolt_map_to_value_map(&bolt_params);
        let result = execute_query(&store, "default", "RETURN $p AS p", params, None).unwrap();
        let (records, _, _, _) = result_to_records(&result, &store, "default");
        assert_eq!(
            records[0][0],
            BoltValue::Structure {
                tag: 0x58,
                fields: vec![
                    BoltValue::Int(7203),
                    BoltValue::Float(1.5),
                    BoltValue::Float(2.5)
                ]
            }
        );
    }

    #[test]
    fn bolt_structure_unknown_tag_returns_null() {
        let val = bolt_structure_to_gql(0xFF, &[BoltValue::Int(1)]);
        assert_eq!(val, Value::Null);
    }
}
