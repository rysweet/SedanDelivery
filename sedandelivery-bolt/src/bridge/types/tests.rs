use super::*;
use cypher_core::storage::InMemoryGraphStore;
use cypher_core::types::value::{Crs, GeoPoint};
use cypher_core::types::{CypherDuration, Value};
use indexmap::IndexMap;

use crate::packstream::value::BoltValue;
use crate::protocol::tags;

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
    let bolt = super::temporal::date_to_bolt(&extreme);
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
    let bolt = super::temporal::local_datetime_to_bolt(&extreme_dt);
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

// ── path_to_bolt: reverse direction & repeated edges ─────────────────────

#[test]
fn path_to_bolt_reverse_direction_uses_negative_index() {
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let n1 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let n2 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    // Edge from n1 -> n2 stored, but path traverses n2 -> n1 (reverse).
    let e1 = store
        .create_edge("test", n1, n2, "KNOWS".to_string(), IndexMap::new())
        .unwrap();
    let bolt = path_to_bolt(&[n2, n1], &[e1], &store, "test");
    let BoltValue::Structure { fields, .. } = bolt else {
        panic!("expected Structure");
    };
    let BoltValue::List(indices) = &fields[2] else {
        panic!("expected indices list");
    };
    // First index is the rel-index (-1 because reverse direction).
    assert_eq!(indices[0], BoltValue::Int(-1));
}

#[test]
fn path_to_bolt_with_missing_edge_uses_empty_relationship() {
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let n1 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let n2 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let bogus = cypher_core::storage::EdgeId(10_000);
    let bolt = path_to_bolt(&[n1, n2], &[bogus], &store, "test");
    let BoltValue::Structure { fields, .. } = bolt else {
        panic!("expected Structure");
    };
    let BoltValue::List(rels) = &fields[1] else {
        panic!("expected rels list");
    };
    assert_eq!(rels.len(), 1);
    let BoltValue::Structure { tag, fields } = &rels[0] else {
        panic!("expected unbound rel");
    };
    assert_eq!(*tag, tags::TAG_UNBOUND_RELATIONSHIP);
    assert_eq!(fields[1], BoltValue::String(String::new()));
}

#[test]
fn path_to_bolt_repeated_edge_dedupes() {
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let n1 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let n2 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let e1 = store
        .create_edge("test", n1, n2, "REL".into(), IndexMap::new())
        .unwrap();
    // Walk n1 -> n2 -> n1 reusing the same edge.
    let bolt = path_to_bolt(&[n1, n2, n1], &[e1, e1], &store, "test");
    let BoltValue::Structure { fields, .. } = bolt else {
        panic!("expected Structure");
    };
    let BoltValue::List(rels) = &fields[1] else {
        panic!("expected rels list");
    };
    assert_eq!(rels.len(), 1, "duplicate edges should appear once");
    let BoltValue::List(indices) = &fields[2] else {
        panic!("expected indices list");
    };
    // 2 steps => 4 entries.
    assert_eq!(indices.len(), 4);
}

// ── path_to_bolt_cached ──────────────────────────────────────────────────

#[test]
fn path_to_bolt_cached_uses_cached_data() {
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let n1 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let n2 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let e1 = store
        .create_edge("test", n1, n2, "REL".into(), IndexMap::new())
        .unwrap();

    let cache = BoltGraphCache::prefetch(&store, "test", &[n1, n2], &[e1]);
    let bolt = path_to_bolt_cached(&[n1, n2], &[e1], &cache, &store, "test");
    let BoltValue::Structure { tag, fields } = bolt else {
        panic!("expected Structure");
    };
    assert_eq!(tag, tags::TAG_PATH);
    let BoltValue::List(nodes) = &fields[0] else {
        panic!("expected nodes list");
    };
    assert_eq!(nodes.len(), 2);
}

#[test]
fn path_to_bolt_cached_falls_back_when_edge_missing_from_cache() {
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let n1 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let n2 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let e1 = store
        .create_edge("test", n1, n2, "REL".into(), IndexMap::new())
        .unwrap();
    // Empty cache → path_to_bolt_cached should fall back to path_to_bolt.
    let cache = BoltGraphCache::prefetch(&store, "test", &[], &[]);
    let bolt = path_to_bolt_cached(&[n1, n2], &[e1], &cache, &store, "test");
    assert!(matches!(bolt, BoltValue::Structure { tag, .. } if tag == tags::TAG_PATH));
}

#[test]
fn path_to_bolt_cached_handles_missing_edge_in_store_via_cache() {
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let n1 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let n2 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let bogus = cypher_core::storage::EdgeId(99_999);
    // prefetch reports None for the bogus edge.
    let cache = BoltGraphCache::prefetch(&store, "test", &[n1, n2], &[bogus]);
    let bolt = path_to_bolt_cached(&[n1, n2], &[bogus], &cache, &store, "test");
    let BoltValue::Structure { fields, .. } = bolt else {
        panic!("expected Structure");
    };
    let BoltValue::List(rels) = &fields[1] else {
        panic!("expected rels list");
    };
    let BoltValue::Structure {
        fields: rel_fields, ..
    } = &rels[0]
    else {
        panic!("expected unbound rel");
    };
    assert_eq!(rel_fields[1], BoltValue::String(String::new()));
}

// ── BoltGraphCache & helpers ─────────────────────────────────────────────

#[test]
fn bolt_graph_cache_prefetch_with_empty_inputs_is_empty() {
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let cache = BoltGraphCache::prefetch(&store, "test", &[], &[]);
    assert!(cache.node(NodeId(0)).is_none());
    assert!(cache.edge(cypher_core::storage::EdgeId(0)).is_none());
}

#[test]
fn bolt_graph_cache_returns_some_none_for_missing_ids() {
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let bogus_node = NodeId(424242);
    let bogus_edge = cypher_core::storage::EdgeId(424242);
    let cache = BoltGraphCache::prefetch(&store, "test", &[bogus_node], &[bogus_edge]);
    assert!(matches!(cache.node(bogus_node), Some(None)));
    assert!(matches!(cache.edge(bogus_edge), Some(None)));
}

#[test]
fn u64_to_bolt_int_wraps_overflow_to_null() {
    use super::u64_to_bolt_int;
    assert_eq!(u64_to_bolt_int(0, "ctx"), BoltValue::Int(0));
    assert_eq!(
        u64_to_bolt_int(i64::MAX as u64, "ctx"),
        BoltValue::Int(i64::MAX)
    );
    assert_eq!(u64_to_bolt_int(u64::MAX, "ctx"), BoltValue::Null);
}

// ── node_to_bolt_cached / edge_to_bolt_cached ────────────────────────────

#[test]
fn node_to_bolt_cached_serves_from_cache() {
    use super::entities::node_to_bolt_cached;
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    let n = store
        .create_node("test", vec!["Person".into()], props)
        .unwrap();
    let cache = BoltGraphCache::prefetch(&store, "test", &[n], &[]);
    let bolt = node_to_bolt_cached(n, &cache, &store, "test");
    let BoltValue::Structure { tag, fields } = bolt else {
        panic!("expected Structure");
    };
    assert_eq!(tag, tags::TAG_NODE);
    assert_eq!(fields[0], BoltValue::Int(n.0 as i64));
}

#[test]
fn node_to_bolt_cached_returns_null_for_cached_missing_node() {
    use super::entities::node_to_bolt_cached;
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let bogus = NodeId(7777);
    let cache = BoltGraphCache::prefetch(&store, "test", &[bogus], &[]);
    let bolt = node_to_bolt_cached(bogus, &cache, &store, "test");
    assert_eq!(bolt, BoltValue::Null);
}

#[test]
fn node_to_bolt_cached_falls_back_to_store_when_not_in_cache() {
    use super::entities::node_to_bolt_cached;
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let n = store.create_node("test", vec![], IndexMap::new()).unwrap();
    // Empty cache — should fall back to node_to_bolt.
    let cache = BoltGraphCache::prefetch(&store, "test", &[], &[]);
    let bolt = node_to_bolt_cached(n, &cache, &store, "test");
    assert!(matches!(bolt, BoltValue::Structure { .. }));
}

#[test]
fn edge_to_bolt_cached_serves_from_cache() {
    use super::entities::edge_to_bolt_cached;
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let n1 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let n2 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let e = store
        .create_edge("test", n1, n2, "T".into(), IndexMap::new())
        .unwrap();
    let cache = BoltGraphCache::prefetch(&store, "test", &[n1, n2], &[e]);
    let bolt = edge_to_bolt_cached(e, &cache, &store, "test");
    let BoltValue::Structure { tag, .. } = bolt else {
        panic!("expected Structure");
    };
    assert_eq!(tag, tags::TAG_RELATIONSHIP);
}

#[test]
fn edge_to_bolt_cached_returns_null_for_cached_missing_edge() {
    use super::entities::edge_to_bolt_cached;
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let bogus = cypher_core::storage::EdgeId(7777);
    let cache = BoltGraphCache::prefetch(&store, "test", &[], &[bogus]);
    let bolt = edge_to_bolt_cached(bogus, &cache, &store, "test");
    assert_eq!(bolt, BoltValue::Null);
}

#[test]
fn edge_to_bolt_cached_falls_back_to_store_when_not_in_cache() {
    use super::entities::edge_to_bolt_cached;
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    let n1 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let n2 = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let e = store
        .create_edge("test", n1, n2, "T".into(), IndexMap::new())
        .unwrap();
    let cache = BoltGraphCache::prefetch(&store, "test", &[], &[]);
    let bolt = edge_to_bolt_cached(e, &cache, &store, "test");
    assert!(matches!(bolt, BoltValue::Structure { .. }));
}
