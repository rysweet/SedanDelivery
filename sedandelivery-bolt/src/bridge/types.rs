use std::collections::HashMap;

use gql_core::storage::{EdgeId, GraphStore, NodeId};
use gql_core::types::value::{Crs, GeoPoint};
use gql_core::types::Value;
use indexmap::IndexMap;

use crate::packstream::value::BoltValue;
use crate::protocol::tags;

// ── ID conversion helper ─────────────────────────────────────────────────────

/// Convert a `u64` storage ID to an `i64` for the Bolt wire format.
/// Returns an error message and `BoltValue::Null` if the ID exceeds `i64::MAX`.
fn u64_to_bolt_int(id: u64, context: &str) -> BoltValue {
    match i64::try_from(id) {
        Ok(v) => BoltValue::Int(v),
        Err(_) => {
            tracing::error!(id, context, "ID exceeds i64::MAX, cannot encode for Bolt");
            BoltValue::Null
        }
    }
}

// ── Node / Edge / Path conversion ────────────────────────────────────────────

/// Convert a node to a Bolt Structure (tag 0x4E).
///
/// Fields: [id: Int, labels: List<String>, properties: Map]
///
/// Returns `BoltValue::Null` if the node is not found in the store.
pub fn node_to_bolt(id: NodeId, store: &dyn GraphStore, graph: &str) -> BoltValue {
    let node_data = match store.get_node(graph, id) {
        Ok(Some(data)) => data,
        Ok(None) => return BoltValue::Null,
        Err(e) => {
            tracing::error!(node_id = id.0, graph = graph, error = %e, "node_to_bolt: store lookup failed");
            return BoltValue::Null;
        }
    };

    let labels = BoltValue::List(
        node_data
            .labels
            .iter()
            .map(|l| BoltValue::String(l.clone()))
            .collect(),
    );

    let props = props_to_bolt_map(&node_data.properties);

    BoltValue::Structure {
        tag: tags::TAG_NODE,
        fields: vec![u64_to_bolt_int(id.0, "node_to_bolt"), labels, props],
    }
}

/// Convert an edge to a Bolt Structure (tag 0x52).
///
/// Fields: [id: Int, startNodeId: Int, endNodeId: Int, type: String, properties: Map]
///
/// Returns `BoltValue::Null` if the edge is not found in the store.
pub fn edge_to_bolt(id: EdgeId, store: &dyn GraphStore, graph: &str) -> BoltValue {
    let edge_data = match store.get_edge(graph, id) {
        Ok(Some(data)) => data,
        Ok(None) => return BoltValue::Null,
        Err(e) => {
            tracing::error!(edge_id = id.0, graph = graph, error = %e, "edge_to_bolt: store lookup failed");
            return BoltValue::Null;
        }
    };

    let props = props_to_bolt_map(&edge_data.properties);

    BoltValue::Structure {
        tag: tags::TAG_RELATIONSHIP,
        fields: vec![
            u64_to_bolt_int(id.0, "edge_to_bolt:id"),
            u64_to_bolt_int(edge_data.src.0, "edge_to_bolt:src"),
            u64_to_bolt_int(edge_data.dst.0, "edge_to_bolt:dst"),
            BoltValue::String(edge_data.rel_type.clone()),
            props,
        ],
    }
}

/// Convert a path to a Bolt Structure (tag 0x50).
///
/// Fields: [nodes: List<Node>, rels: List<UnboundRelationship>, indices: List<Int>]
///
/// Unbound relationships use tag 0x72 with fields: [id, type, properties].
///
/// The index sequence encodes the traversal order: for each step, a positive
/// relationship index (1-based) means forward traversal, a negative index means
/// reverse. The subsequent node index (0-based into unique-nodes list) follows.
pub fn path_to_bolt(
    node_ids: &[NodeId],
    edge_ids: &[EdgeId],
    store: &dyn GraphStore,
    graph: &str,
) -> BoltValue {
    // Build the unique-nodes list (preserving first-occurrence order).
    let mut unique_nodes: Vec<NodeId> = Vec::new();
    let mut node_index_map: HashMap<NodeId, usize> = HashMap::new();
    for &nid in node_ids {
        if let std::collections::hash_map::Entry::Vacant(e) = node_index_map.entry(nid) {
            e.insert(unique_nodes.len());
            unique_nodes.push(nid);
        }
    }

    let bolt_unique_nodes: Vec<BoltValue> = unique_nodes
        .iter()
        .map(|&nid| node_to_bolt(nid, store, graph))
        .collect();

    // Build unbound relationships (tag 0x72: id, type, properties).
    let mut unbound_rels: Vec<BoltValue> = Vec::new();
    let mut edge_rel_index: Vec<usize> = Vec::new();
    let mut seen_edges: HashMap<EdgeId, usize> = HashMap::new();
    for &eid in edge_ids {
        if let Some(&idx) = seen_edges.get(&eid) {
            edge_rel_index.push(idx);
        } else {
            let idx = unbound_rels.len();
            seen_edges.insert(eid, idx);
            edge_rel_index.push(idx);

            let rel = match store.get_edge(graph, eid) {
                Ok(Some(data)) => BoltValue::Structure {
                    tag: tags::TAG_UNBOUND_RELATIONSHIP,
                    fields: vec![
                        u64_to_bolt_int(eid.0, "path_to_bolt:rel_id"),
                        BoltValue::String(data.rel_type.clone()),
                        props_to_bolt_map(&data.properties),
                    ],
                },
                Ok(None) => {
                    tracing::warn!(
                        edge_id = eid.0,
                        graph = graph,
                        "path_to_bolt: edge not found in store, using empty relationship"
                    );
                    BoltValue::Structure {
                        tag: tags::TAG_UNBOUND_RELATIONSHIP,
                        fields: vec![
                            u64_to_bolt_int(eid.0, "path_to_bolt:rel_id"),
                            BoltValue::String(String::new()),
                            BoltValue::Map(vec![]),
                        ],
                    }
                }
                Err(e) => {
                    tracing::error!(edge_id = eid.0, graph = graph, error = %e, "path_to_bolt: store lookup failed for edge, using empty relationship");
                    BoltValue::Structure {
                        tag: tags::TAG_UNBOUND_RELATIONSHIP,
                        fields: vec![
                            u64_to_bolt_int(eid.0, "path_to_bolt:rel_id"),
                            BoltValue::String(String::new()),
                            BoltValue::Map(vec![]),
                        ],
                    }
                }
            };
            unbound_rels.push(rel);
        }
    }

    // Build the index sequence.
    // For each step i: edge_ids[i] connects node_ids[i] -> node_ids[i+1].
    // Relationship index is 1-based into unbound_rels.
    // If the edge's src matches node_ids[i], we traverse forward (+rel_idx).
    // If the edge's dst matches node_ids[i], we traverse backward (-rel_idx).
    let mut indices: Vec<BoltValue> = Vec::new();
    for i in 0..edge_ids.len() {
        let rel_1based = (edge_rel_index[i] as i64) + 1;
        let from_node = node_ids[i];

        // Determine direction by checking edge src/dst against the from-node.
        let rel_index = match store.get_edge(graph, edge_ids[i]) {
            Ok(Some(data)) => {
                if data.src == from_node {
                    rel_1based
                } else {
                    -rel_1based
                }
            }
            Ok(None) => {
                tracing::warn!(edge_id = edge_ids[i].0, graph = graph, "path_to_bolt: edge not found when determining direction, defaulting to forward");
                rel_1based
            }
            Err(e) => {
                tracing::error!(edge_id = edge_ids[i].0, graph = graph, error = %e, "path_to_bolt: store lookup failed when determining direction, defaulting to forward");
                rel_1based
            }
        };
        indices.push(BoltValue::Int(rel_index));

        let to_node = node_ids[i + 1];
        let node_idx = match node_index_map.get(&to_node) {
            Some(&idx) => idx as i64,
            None => {
                tracing::warn!(
                    node_id = to_node.0,
                    "node_index_map missing to_node in path encoding, falling back to index 0"
                );
                0
            }
        };
        indices.push(BoltValue::Int(node_idx));
    }

    BoltValue::Structure {
        tag: tags::TAG_PATH,
        fields: vec![
            BoltValue::List(bolt_unique_nodes),
            BoltValue::List(unbound_rels),
            BoltValue::List(indices),
        ],
    }
}

// ── Property / scalar conversion ─────────────────────────────────────────────

/// Convert a gql-core property map to a `BoltValue::Map`.
///
/// Only scalar values appear in property maps (no Node/Edge/Path references).
pub fn props_to_bolt_map(props: &IndexMap<String, Value>) -> BoltValue {
    let entries: Vec<(String, BoltValue)> = props
        .iter()
        .map(|(k, v)| (k.clone(), scalar_value_to_bolt(v)))
        .collect();
    BoltValue::Map(entries)
}

/// Convert a gql-core scalar Value to a BoltValue.
///
/// Handles Null, Bool, Int, Float, Str, List, Map, and temporal/spatial types.
/// Does NOT handle Node, Edge, or Path -- those require a store lookup and are
/// handled separately by `node_to_bolt`, `edge_to_bolt`, `path_to_bolt`.
pub fn scalar_value_to_bolt(val: &Value) -> BoltValue {
    match val {
        Value::Null => BoltValue::Null,
        Value::Bool(b) => BoltValue::Bool(*b),
        Value::Int(i) => BoltValue::Int(*i),
        Value::Float(f) => BoltValue::Float(*f),
        Value::Str(s) => BoltValue::String(s.to_string()),
        Value::List(items) => BoltValue::List(items.iter().map(scalar_value_to_bolt).collect()),
        Value::Map(map) => indexmap_to_bolt_map(map),

        // ── Temporal types ───────────────────────────────────────────────
        Value::Date(d) => date_to_bolt(d),
        Value::LocalTime(t) => local_time_to_bolt(t),
        Value::Time(t, offset_secs) => zoned_time_to_bolt(t, *offset_secs),
        Value::LocalDateTime(dt) => local_datetime_to_bolt(dt),
        Value::DateTime(zoned) => datetime_to_bolt(zoned),
        Value::Duration(dur) => duration_to_bolt(dur),

        // ── Spatial types ────────────────────────────────────────────────
        Value::Point(p) => point_to_bolt(p),

        // Bytes: direct mapping to Bolt Bytes
        Value::Bytes(b) => BoltValue::Bytes(b.clone()),

        // Node/Edge/Path should not appear in scalar context; treat as Null
        // rather than panicking for robustness.
        Value::Node(_) | Value::Edge(_) | Value::Path { .. } => BoltValue::Null,
    }
}

// ── IndexMap conversion helper ───────────────────────────────────────────────

fn indexmap_to_bolt_map(map: &IndexMap<Box<str>, Value>) -> BoltValue {
    let entries: Vec<(String, BoltValue)> = map
        .iter()
        .map(|(k, v)| (k.to_string(), scalar_value_to_bolt(v)))
        .collect();
    BoltValue::Map(entries)
}

// ── Temporal encodings ───────────────────────────────────────────────────────

/// Date: 1 field (days since 1970-01-01 epoch).
fn date_to_bolt(d: &jiff::civil::Date) -> BoltValue {
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
fn local_time_to_bolt(t: &jiff::civil::Time) -> BoltValue {
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
fn local_datetime_to_bolt(dt: &jiff::civil::DateTime) -> BoltValue {
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
fn zoned_time_to_bolt(t: &jiff::civil::Time, offset_secs: i32) -> BoltValue {
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
fn datetime_to_bolt(zoned: &jiff::Zoned) -> BoltValue {
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
fn duration_to_bolt(dur: &gql_core::types::CypherDuration) -> BoltValue {
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

/// Point2D or Point3D encoding based on the presence of a z coordinate.
fn point_to_bolt(p: &GeoPoint) -> BoltValue {
    let srid: i64 = match p.crs {
        Crs::Wgs84 => 4326,
        Crs::Wgs843d => 4979,
        Crs::Cartesian => 7203,
        Crs::Cartesian3d => 9157,
    };
    if let Some(z) = p.z {
        BoltValue::Structure {
            tag: tags::TAG_POINT_3D,
            fields: vec![
                BoltValue::Int(srid),
                BoltValue::Float(p.x),
                BoltValue::Float(p.y),
                BoltValue::Float(z),
            ],
        }
    } else {
        BoltValue::Structure {
            tag: tags::TAG_POINT_2D,
            fields: vec![
                BoltValue::Int(srid),
                BoltValue::Float(p.x),
                BoltValue::Float(p.y),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gql_core::storage::InMemoryGraphStore;
    use gql_core::types::CypherDuration;

    #[test]
    fn null_converts_to_null() {
        assert_eq!(scalar_value_to_bolt(&Value::Null), BoltValue::Null);
    }

    #[test]
    fn bool_converts() {
        assert_eq!(
            scalar_value_to_bolt(&Value::Bool(true)),
            BoltValue::Bool(true)
        );
        assert_eq!(
            scalar_value_to_bolt(&Value::Bool(false)),
            BoltValue::Bool(false)
        );
    }

    #[test]
    fn int_converts() {
        assert_eq!(scalar_value_to_bolt(&Value::Int(42)), BoltValue::Int(42));
        assert_eq!(scalar_value_to_bolt(&Value::Int(-1)), BoltValue::Int(-1));
    }

    #[test]
    fn float_converts() {
        assert_eq!(
            scalar_value_to_bolt(&Value::Float(2.72)),
            BoltValue::Float(2.72)
        );
    }

    #[test]
    fn string_converts() {
        assert_eq!(
            scalar_value_to_bolt(&Value::Str("hello".into())),
            BoltValue::String("hello".to_string())
        );
    }

    #[test]
    fn list_converts() {
        let val = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let bolt = scalar_value_to_bolt(&val);
        assert_eq!(
            bolt,
            BoltValue::List(vec![BoltValue::Int(1), BoltValue::Int(2)])
        );
    }

    #[test]
    fn map_converts() {
        let mut map = IndexMap::new();
        map.insert(Box::from("key"), Value::Int(99));
        let val = Value::Map(map);
        let bolt = scalar_value_to_bolt(&val);
        if let BoltValue::Map(entries) = bolt {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].0, "key");
            assert_eq!(entries[0].1, BoltValue::Int(99));
        } else {
            panic!("expected BoltValue::Map");
        }
    }

    #[test]
    fn date_encoding() {
        // 2024-01-15 = 19737 days since 1970-01-01
        let d = jiff::civil::Date::new(2024, 1, 15).unwrap();
        let bolt = scalar_value_to_bolt(&Value::Date(d));
        if let BoltValue::Structure { tag, fields } = &bolt {
            assert_eq!(*tag, 0x44);
            assert_eq!(fields.len(), 1);
            if let BoltValue::Int(days) = &fields[0] {
                assert_eq!(*days, 19737);
            } else {
                panic!("expected Int for days");
            }
        } else {
            panic!("expected Structure for Date");
        }
    }

    #[test]
    fn local_time_encoding() {
        // 12:30:00 = 45000 seconds * 1_000_000_000 = 45_000_000_000_000 nanos
        let t = jiff::civil::Time::new(12, 30, 0, 0).unwrap();
        let bolt = scalar_value_to_bolt(&Value::LocalTime(t));
        if let BoltValue::Structure { tag, fields } = &bolt {
            assert_eq!(*tag, 0x74);
            assert_eq!(fields.len(), 1);
            if let BoltValue::Int(nanos) = &fields[0] {
                assert_eq!(*nanos, 45_000_000_000_000);
            } else {
                panic!("expected Int for nanos");
            }
        } else {
            panic!("expected Structure for LocalTime");
        }
    }

    #[test]
    fn duration_encoding() {
        let dur = CypherDuration {
            months: 14,
            days: 3,
            nanos: 5_500_000_000,
        };
        let bolt = scalar_value_to_bolt(&Value::Duration(dur));
        if let BoltValue::Structure { tag, fields } = &bolt {
            assert_eq!(*tag, 0x45);
            assert_eq!(fields.len(), 4);
            assert_eq!(fields[0], BoltValue::Int(14));
            assert_eq!(fields[1], BoltValue::Int(3));
            assert_eq!(fields[2], BoltValue::Int(5)); // seconds
            assert_eq!(fields[3], BoltValue::Int(500_000_000)); // nanos
        } else {
            panic!("expected Structure for Duration");
        }
    }

    #[test]
    fn point_wgs84_encoding() {
        let p = GeoPoint {
            x: -73.95,
            y: 40.78,
            z: None,
            crs: Crs::Wgs84,
        };
        let bolt = scalar_value_to_bolt(&Value::Point(p));
        if let BoltValue::Structure { tag, fields } = &bolt {
            assert_eq!(*tag, 0x58);
            assert_eq!(fields.len(), 3);
            assert_eq!(fields[0], BoltValue::Int(4326));
            assert_eq!(fields[1], BoltValue::Float(-73.95));
            assert_eq!(fields[2], BoltValue::Float(40.78));
        } else {
            panic!("expected Structure for Point");
        }
    }

    #[test]
    fn point_cartesian_encoding() {
        let p = GeoPoint {
            x: 1.0,
            y: 2.0,
            z: None,
            crs: Crs::Cartesian,
        };
        let bolt = scalar_value_to_bolt(&Value::Point(p));
        if let BoltValue::Structure { tag, fields } = &bolt {
            assert_eq!(*tag, 0x58);
            assert_eq!(fields[0], BoltValue::Int(7203));
        } else {
            panic!("expected Structure for Point");
        }
    }

    #[test]
    fn local_datetime_epoch_seconds_correct() {
        // 2024-01-15T10:30:45.123456789
        let dt = jiff::civil::DateTime::new(2024, 1, 15, 10, 30, 45, 123456789).unwrap();
        let bolt = scalar_value_to_bolt(&Value::LocalDateTime(dt));
        if let BoltValue::Structure { tag, fields } = &bolt {
            assert_eq!(*tag, 0x64);
            assert_eq!(fields.len(), 2);
            // Expected: 19737 * 86400 + 10*3600 + 30*60 + 45 = 1705311045
            let expected_secs = 19737_i64 * 86400 + 10 * 3600 + 30 * 60 + 45;
            assert_eq!(fields[0], BoltValue::Int(expected_secs), "epoch seconds");
            assert_eq!(
                fields[1],
                BoltValue::Int(123456789),
                "nanosecond adjustment"
            );
        } else {
            panic!("expected Structure for LocalDateTime");
        }
    }

    #[test]
    fn local_datetime_no_subsec() {
        // 2024-01-15T10:30:00 (no sub-second)
        let dt = jiff::civil::DateTime::new(2024, 1, 15, 10, 30, 0, 0).unwrap();
        let bolt = scalar_value_to_bolt(&Value::LocalDateTime(dt));
        if let BoltValue::Structure { tag, fields } = &bolt {
            assert_eq!(*tag, 0x64);
            let expected_secs = 19737_i64 * 86400 + 10 * 3600 + 30 * 60;
            assert_eq!(fields[0], BoltValue::Int(expected_secs), "epoch seconds");
            assert_eq!(fields[1], BoltValue::Int(0), "nanosecond adjustment");
        } else {
            panic!("expected Structure for LocalDateTime");
        }
    }

    #[test]
    fn datetime_zoned_uses_tag_0x69() {
        // Zoned datetime with IANA timezone should use tag 0x69 (UTC mode, lowercase 'i')
        let zoned = jiff::civil::DateTime::new(2024, 1, 15, 10, 30, 0, 0)
            .unwrap()
            .to_zoned(jiff::tz::TimeZone::get("UTC").unwrap())
            .unwrap();
        let bolt = scalar_value_to_bolt(&Value::DateTime(Box::new(zoned)));
        if let BoltValue::Structure { tag, fields } = &bolt {
            assert_eq!(
                *tag, 0x69,
                "UTC-mode DateTimeZoneId tag should be 0x69 (lowercase 'i')"
            );
            assert_eq!(fields.len(), 3);
            // epoch seconds for 2024-01-15T10:30:00Z
            let expected_secs = 19737_i64 * 86400 + 10 * 3600 + 30 * 60;
            assert_eq!(fields[0], BoltValue::Int(expected_secs));
            assert_eq!(fields[1], BoltValue::Int(0)); // no sub-second nanos
            assert_eq!(fields[2], BoltValue::String("UTC".to_string()));
        } else {
            panic!("expected Structure for DateTime");
        }
    }

    #[test]
    fn datetime_fixed_offset_uses_tag_0x49() {
        // DateTime with fixed offset (no IANA name) should use tag 0x49 (uppercase 'I')
        let zoned = jiff::civil::DateTime::new(2024, 1, 15, 10, 30, 0, 0)
            .unwrap()
            .to_zoned(jiff::tz::TimeZone::fixed(jiff::tz::offset(3)))
            .unwrap();
        let bolt = scalar_value_to_bolt(&Value::DateTime(Box::new(zoned)));
        if let BoltValue::Structure { tag, fields } = &bolt {
            assert_eq!(
                *tag, 0x49,
                "UTC-mode fixed-offset DateTime should use tag 0x49 (uppercase 'I')"
            );
            assert_eq!(fields.len(), 3);
            // epoch seconds for 2024-01-15T10:30:00+03:00 = 2024-01-15T07:30:00Z
            let expected_secs = 19737_i64 * 86400 + 7 * 3600 + 30 * 60;
            assert_eq!(fields[0], BoltValue::Int(expected_secs));
            assert_eq!(fields[1], BoltValue::Int(0)); // no sub-second nanos
            assert_eq!(fields[2], BoltValue::Int(10800)); // +3 hours = 10800 seconds
        } else {
            panic!("expected Structure for DateTime");
        }
    }

    #[test]
    fn node_graph_reference_returns_null() {
        // Node/Edge/Path in scalar context should return Null
        assert_eq!(
            scalar_value_to_bolt(&Value::Node(NodeId(1))),
            BoltValue::Null
        );
        assert_eq!(
            scalar_value_to_bolt(&Value::Edge(EdgeId(1))),
            BoltValue::Null
        );
    }

    #[test]
    fn empty_props_map() {
        let props = IndexMap::new();
        let bolt = props_to_bolt_map(&props);
        assert_eq!(bolt, BoltValue::Map(vec![]));
    }

    #[test]
    fn node_to_bolt_returns_structure() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        let id = store
            .create_node(
                "test",
                vec!["Person".to_string()],
                IndexMap::from([("name".to_string(), Value::Str("Alice".into()))]),
            )
            .unwrap();

        let bolt = node_to_bolt(id, &store, "test");
        if let BoltValue::Structure { tag, fields } = &bolt {
            assert_eq!(*tag, 0x4E);
            assert_eq!(fields.len(), 3);
            assert_eq!(fields[0], BoltValue::Int(id.0 as i64));
            // labels
            if let BoltValue::List(labels) = &fields[1] {
                assert_eq!(labels.len(), 1);
                assert_eq!(labels[0], BoltValue::String("Person".to_string()));
            } else {
                panic!("expected List for labels");
            }
        } else {
            panic!("expected Structure for node");
        }
    }

    #[test]
    fn node_to_bolt_not_found_returns_null() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        let bolt = node_to_bolt(NodeId(999), &store, "test");
        assert_eq!(bolt, BoltValue::Null);
    }

    #[test]
    fn edge_to_bolt_returns_structure() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        let n1 = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let n2 = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let eid = store
            .create_edge("test", n1, n2, "KNOWS".to_string(), IndexMap::new())
            .unwrap();

        let bolt = edge_to_bolt(eid, &store, "test");
        if let BoltValue::Structure { tag, fields } = &bolt {
            assert_eq!(*tag, 0x52);
            assert_eq!(fields.len(), 5);
            assert_eq!(fields[0], BoltValue::Int(eid.0 as i64));
            assert_eq!(fields[1], BoltValue::Int(n1.0 as i64));
            assert_eq!(fields[2], BoltValue::Int(n2.0 as i64));
            assert_eq!(fields[3], BoltValue::String("KNOWS".to_string()));
        } else {
            panic!("expected Structure for edge");
        }
    }

    #[test]
    fn path_to_bolt_simple() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        let n1 = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let n2 = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let e1 = store
            .create_edge("test", n1, n2, "KNOWS".to_string(), IndexMap::new())
            .unwrap();

        let bolt = path_to_bolt(&[n1, n2], &[e1], &store, "test");
        if let BoltValue::Structure { tag, fields } = &bolt {
            assert_eq!(*tag, 0x50);
            assert_eq!(fields.len(), 3);
            // unique nodes
            if let BoltValue::List(nodes) = &fields[0] {
                assert_eq!(nodes.len(), 2);
            } else {
                panic!("expected List for nodes");
            }
            // unbound rels
            if let BoltValue::List(rels) = &fields[1] {
                assert_eq!(rels.len(), 1);
                if let BoltValue::Structure { tag, .. } = &rels[0] {
                    assert_eq!(*tag, 0x72);
                }
            } else {
                panic!("expected List for rels");
            }
            // indices
            if let BoltValue::List(indices) = &fields[2] {
                assert_eq!(indices.len(), 2); // one rel index + one node index
            } else {
                panic!("expected List for indices");
            }
        } else {
            panic!("expected Structure for path");
        }
    }

    // ── Extreme date encoding tests ─────────────────────────────────────────

    #[test]
    fn date_to_bolt_extreme_date_does_not_panic() {
        // jiff::civil::Date::MAX may overflow the since(epoch) span computation.
        // Verify we get Null (graceful fallback) instead of a panic.
        let extreme = jiff::civil::Date::MAX;
        let bolt = date_to_bolt(&extreme);
        match bolt {
            BoltValue::Null => {} // graceful fallback
            BoltValue::Structure { tag, .. } => assert_eq!(tag, tags::TAG_DATE),
            other => panic!("unexpected BoltValue: {other:?}"),
        }
    }

    #[test]
    fn local_datetime_to_bolt_extreme_date_does_not_panic() {
        let extreme_dt = jiff::civil::DateTime::from_parts(
            jiff::civil::Date::MAX,
            jiff::civil::Time::new(0, 0, 0, 0).unwrap(),
        );
        let bolt = local_datetime_to_bolt(&extreme_dt);
        match bolt {
            BoltValue::Null => {} // graceful fallback
            BoltValue::Structure { tag, .. } => assert_eq!(tag, tags::TAG_LOCAL_DATE_TIME),
            other => panic!("unexpected BoltValue: {other:?}"),
        }
    }

    #[test]
    fn duration_encoding_negative_nanos_normalized() {
        // -1.5 seconds = -1_500_000_000 nanos.
        // Bolt spec requires nanoseconds in 0..999_999_999, so this should
        // become seconds=-2, nanos=500_000_000 (i.e., -2s + 0.5s = -1.5s).
        let dur = CypherDuration {
            months: 0,
            days: 0,
            nanos: -1_500_000_000,
        };
        let bolt = scalar_value_to_bolt(&Value::Duration(dur));
        if let BoltValue::Structure { tag, fields } = &bolt {
            assert_eq!(*tag, 0x45);
            assert_eq!(fields.len(), 4);
            assert_eq!(fields[0], BoltValue::Int(0)); // months
            assert_eq!(fields[1], BoltValue::Int(0)); // days
            assert_eq!(fields[2], BoltValue::Int(-2)); // seconds
            assert_eq!(fields[3], BoltValue::Int(500_000_000)); // nanos (non-negative)
        } else {
            panic!("expected Structure for Duration");
        }
    }
}
