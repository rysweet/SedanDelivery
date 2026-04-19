use sedan_client::result::{parse_mutate_response, parse_query_response};
use sedan_client::types::{decode_value, substitute_params, Point, SedanPath};
/// Comprehensive unit tests for sedan-client.
///
/// These tests do NOT require a running SedanDelivery instance.
/// They test parsing, escaping, type decoding, and result handling.
use sedan_client::{MutationStats, QueryResult, Record, SedanValue};

// ---------------------------------------------------------------------------
// 1. MutationStats::parse
// ---------------------------------------------------------------------------

#[test]
fn test_mutation_stats_parse_full() {
    let s = "nodes_created=5 nodes_deleted=1 edges_created=3 edges_deleted=0 properties_set=10 labels_added=2";
    let stats = MutationStats::parse(s);
    assert_eq!(stats.nodes_created, 5);
    assert_eq!(stats.nodes_deleted, 1);
    assert_eq!(stats.edges_created, 3);
    assert_eq!(stats.edges_deleted, 0);
    assert_eq!(stats.properties_set, 10);
    assert_eq!(stats.labels_added, 2);
}

#[test]
fn test_mutation_stats_parse_all_zeros() {
    let s = "nodes_created=0 nodes_deleted=0 edges_created=0 edges_deleted=0 properties_set=0 labels_added=0";
    let stats = MutationStats::parse(s);
    assert_eq!(stats, MutationStats::default());
}

#[test]
fn test_mutation_stats_default() {
    let stats = MutationStats::default();
    assert_eq!(stats.nodes_created, 0);
    assert_eq!(stats.nodes_deleted, 0);
    assert_eq!(stats.edges_created, 0);
    assert_eq!(stats.edges_deleted, 0);
    assert_eq!(stats.properties_set, 0);
    assert_eq!(stats.labels_added, 0);
}

#[test]
fn test_mutation_stats_parse_partial() {
    let s = "nodes_created=3 properties_set=7";
    let stats = MutationStats::parse(s);
    assert_eq!(stats.nodes_created, 3);
    assert_eq!(stats.nodes_deleted, 0);
    assert_eq!(stats.properties_set, 7);
}

#[test]
fn test_mutation_stats_parse_unknown_keys_ignored() {
    let s = "nodes_created=1 unknown_stat=99 edges_created=2";
    let stats = MutationStats::parse(s);
    assert_eq!(stats.nodes_created, 1);
    assert_eq!(stats.edges_created, 2);
}

#[test]
fn test_mutation_stats_parse_empty_string() {
    let stats = MutationStats::parse("");
    assert_eq!(stats, MutationStats::default());
}

#[test]
fn test_mutation_stats_equality() {
    let a = MutationStats {
        nodes_created: 1,
        edges_created: 2,
        ..Default::default()
    };
    let b = MutationStats {
        nodes_created: 1,
        edges_created: 2,
        ..Default::default()
    };
    assert_eq!(a, b);
}

#[test]
fn test_mutation_stats_inequality() {
    let a = MutationStats {
        nodes_created: 1,
        ..Default::default()
    };
    let b = MutationStats {
        nodes_created: 2,
        ..Default::default()
    };
    assert_ne!(a, b);
}

// ---------------------------------------------------------------------------
// 2. Record access
// ---------------------------------------------------------------------------

fn make_record(pairs: &[(&str, serde_json::Value)]) -> Record {
    let mut map = serde_json::Map::new();
    for (k, v) in pairs {
        map.insert(k.to_string(), v.clone());
    }
    Record::new(map)
}

#[test]
fn test_record_get() {
    let r = make_record(&[("name", serde_json::Value::String("Alice".into()))]);
    assert_eq!(
        r.get("name"),
        Some(&serde_json::Value::String("Alice".into()))
    );
    assert_eq!(r.get("missing"), None);
}

#[test]
fn test_record_get_str() {
    let r = make_record(&[("name", serde_json::Value::String("Bob".into()))]);
    assert_eq!(r.get_str("name"), Some("Bob"));
    assert_eq!(r.get_str("missing"), None);
}

#[test]
fn test_record_get_str_not_string() {
    let r = make_record(&[("age", serde_json::json!(30))]);
    assert_eq!(r.get_str("age"), None);
}

#[test]
fn test_record_get_i64() {
    let r = make_record(&[("age", serde_json::json!(30))]);
    assert_eq!(r.get_i64("age"), Some(30));
    assert_eq!(r.get_i64("missing"), None);
}

#[test]
fn test_record_get_f64() {
    let r = make_record(&[("score", serde_json::json!(3.14))]);
    assert_eq!(r.get_f64("score"), Some(3.14));
}

#[test]
fn test_record_get_bool() {
    let r = make_record(&[("active", serde_json::json!(true))]);
    assert_eq!(r.get_bool("active"), Some(true));
    assert_eq!(r.get_bool("missing"), None);
}

#[test]
fn test_record_index_access() {
    let r = make_record(&[
        ("name", serde_json::Value::String("Alice".into())),
        ("age", serde_json::json!(30)),
    ]);
    assert_eq!(r["name"], serde_json::Value::String("Alice".into()));
    assert_eq!(r["age"], serde_json::json!(30));
}

#[test]
#[should_panic]
fn test_record_index_missing_panics() {
    let r = make_record(&[("name", serde_json::Value::String("Alice".into()))]);
    let _ = &r["missing"];
}

#[test]
fn test_record_keys() {
    let r = make_record(&[("b", serde_json::json!(2)), ("a", serde_json::json!(1))]);
    let keys: Vec<&String> = r.keys().collect();
    assert_eq!(keys.len(), 2);
    // serde_json::Map preserves insertion order
    assert!(keys.contains(&&"a".to_string()));
    assert!(keys.contains(&&"b".to_string()));
}

#[test]
fn test_record_data() {
    let r = make_record(&[("x", serde_json::json!(1))]);
    let data = r.data();
    assert_eq!(data.get("x"), Some(&serde_json::json!(1)));
}

// ---------------------------------------------------------------------------
// 3. Parameter substitution
// ---------------------------------------------------------------------------

#[test]
fn test_substitute_null() {
    let result = substitute_params("SET n.x = $val", &[("val", SedanValue::Null)]);
    assert_eq!(result, "SET n.x = null");
}

#[test]
fn test_substitute_bool() {
    let result = substitute_params(
        "SET n.a = $a, n.d = $d",
        &[
            ("a", SedanValue::Bool(true)),
            ("d", SedanValue::Bool(false)),
        ],
    );
    assert_eq!(result, "SET n.a = true, n.d = false");
}

#[test]
fn test_substitute_int() {
    let result = substitute_params("WHERE n.age > $age", &[("age", SedanValue::Int(21))]);
    assert_eq!(result, "WHERE n.age > 21");
}

#[test]
fn test_substitute_negative_int() {
    let result = substitute_params("SET n.x = $val", &[("val", SedanValue::Int(-42))]);
    assert_eq!(result, "SET n.x = -42");
}

#[test]
fn test_substitute_float() {
    let result = substitute_params("SET n.x = $val", &[("val", SedanValue::Float(3.14))]);
    assert!(result.contains("3.14"));
}

#[test]
fn test_substitute_string() {
    let result = substitute_params(
        "MATCH (n {name: $name})",
        &[("name", SedanValue::Str("Alice".into()))],
    );
    assert_eq!(result, "MATCH (n {name: 'Alice'})");
}

#[test]
fn test_substitute_string_with_quotes() {
    let result = substitute_params(
        "MATCH (n {name: $name})",
        &[("name", SedanValue::Str("O'Brien".into()))],
    );
    assert_eq!(result, "MATCH (n {name: 'O\\'Brien'})");
}

#[test]
fn test_substitute_string_with_backslash() {
    let result = substitute_params("SET n.x = $val", &[("val", SedanValue::Str("a\\b".into()))]);
    assert_eq!(result, "SET n.x = 'a\\\\b'");
}

#[test]
fn test_substitute_empty_string() {
    let result = substitute_params("SET n.x = $val", &[("val", SedanValue::Str("".into()))]);
    assert_eq!(result, "SET n.x = ''");
}

#[test]
fn test_substitute_list() {
    let result = substitute_params(
        "WHERE n.id IN $ids",
        &[(
            "ids",
            SedanValue::List(vec![
                SedanValue::Int(1),
                SedanValue::Int(2),
                SedanValue::Int(3),
            ]),
        )],
    );
    assert_eq!(result, "WHERE n.id IN [1, 2, 3]");
}

#[test]
fn test_substitute_nested_list() {
    let result = substitute_params(
        "SET n.x = $val",
        &[(
            "val",
            SedanValue::List(vec![
                SedanValue::Int(1),
                SedanValue::List(vec![SedanValue::Int(2), SedanValue::Int(3)]),
            ]),
        )],
    );
    assert_eq!(result, "SET n.x = [1, [2, 3]]");
}

#[test]
fn test_substitute_empty_list() {
    let result = substitute_params("SET n.x = $val", &[("val", SedanValue::List(vec![]))]);
    assert_eq!(result, "SET n.x = []");
}

#[test]
fn test_substitute_unknown_param_left_intact() {
    let result = substitute_params("$known $unknown", &[("known", SedanValue::Int(42))]);
    assert_eq!(result, "42 $unknown");
}

#[test]
fn test_substitute_no_params() {
    let result = substitute_params("MATCH (n) RETURN n", &[]);
    assert_eq!(result, "MATCH (n) RETURN n");
}

#[test]
fn test_substitute_unicode_string() {
    let result = substitute_params(
        "SET n.name = $name",
        &[("name", SedanValue::Str("\u{1F600} \u{00E9}".into()))],
    );
    // Should contain the escaped Unicode chars in single quotes
    assert!(result.starts_with("SET n.name = '"));
    assert!(result.ends_with('\''));
}

#[test]
fn test_substitute_mixed_list() {
    let result = substitute_params(
        "SET n.x = $val",
        &[(
            "val",
            SedanValue::List(vec![
                SedanValue::Int(1),
                SedanValue::Str("hello".into()),
                SedanValue::Bool(true),
                SedanValue::Null,
            ]),
        )],
    );
    assert_eq!(result, "SET n.x = [1, 'hello', true, null]");
}

// ---------------------------------------------------------------------------
// 4. QueryResult iteration
// ---------------------------------------------------------------------------

#[test]
fn test_query_result_len() {
    let qr = QueryResult::new(
        vec![
            make_record(&[("x", serde_json::json!(1))]),
            make_record(&[("x", serde_json::json!(2))]),
        ],
        MutationStats::default(),
    );
    assert_eq!(qr.len(), 2);
}

#[test]
fn test_query_result_is_empty() {
    let qr = QueryResult::new(vec![], MutationStats::default());
    assert!(qr.is_empty());
}

#[test]
fn test_query_result_not_empty() {
    let qr = QueryResult::new(
        vec![make_record(&[("x", serde_json::json!(1))])],
        MutationStats::default(),
    );
    assert!(!qr.is_empty());
}

#[test]
fn test_query_result_index_access() {
    let qr = QueryResult::new(
        vec![
            make_record(&[("x", serde_json::json!(1))]),
            make_record(&[("x", serde_json::json!(2))]),
        ],
        MutationStats::default(),
    );
    assert_eq!(qr[0]["x"], serde_json::json!(1));
    assert_eq!(qr[1]["x"], serde_json::json!(2));
}

#[test]
fn test_query_result_iter() {
    let qr = QueryResult::new(
        vec![
            make_record(&[("x", serde_json::json!(1))]),
            make_record(&[("x", serde_json::json!(2))]),
        ],
        MutationStats::default(),
    );
    let vals: Vec<i64> = qr.iter().map(|r| r.get_i64("x").unwrap()).collect();
    assert_eq!(vals, vec![1, 2]);
}

#[test]
fn test_query_result_into_iter() {
    let qr = QueryResult::new(
        vec![
            make_record(&[("x", serde_json::json!(1))]),
            make_record(&[("x", serde_json::json!(2))]),
        ],
        MutationStats::default(),
    );
    let vals: Vec<i64> = qr.into_iter().map(|r| r.get_i64("x").unwrap()).collect();
    assert_eq!(vals, vec![1, 2]);
}

#[test]
fn test_query_result_for_loop() {
    let qr = QueryResult::new(
        vec![
            make_record(&[("x", serde_json::json!(10))]),
            make_record(&[("x", serde_json::json!(20))]),
        ],
        MutationStats::default(),
    );
    let mut sum = 0i64;
    for r in &qr {
        sum += r.get_i64("x").unwrap();
    }
    assert_eq!(sum, 30);
}

#[test]
fn test_query_result_stats() {
    let stats = MutationStats {
        nodes_created: 3,
        edges_created: 2,
        ..Default::default()
    };
    let qr = QueryResult::new(vec![], stats.clone());
    assert_eq!(qr.stats().nodes_created, 3);
    assert_eq!(qr.stats().edges_created, 2);
}

#[test]
fn test_query_result_records_slice() {
    let qr = QueryResult::new(
        vec![make_record(&[("x", serde_json::json!(1))])],
        MutationStats::default(),
    );
    let records = qr.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["x"], serde_json::json!(1));
}

// ---------------------------------------------------------------------------
// 5. Tagged value decoding
// ---------------------------------------------------------------------------

#[test]
fn test_decode_plain_string() {
    let v = serde_json::json!("hello");
    assert_eq!(decode_value(&v), serde_json::json!("hello"));
}

#[test]
fn test_decode_null() {
    let v = serde_json::Value::Null;
    assert_eq!(decode_value(&v), serde_json::Value::Null);
}

#[test]
fn test_decode_bool() {
    assert_eq!(
        decode_value(&serde_json::json!(true)),
        serde_json::json!(true)
    );
    assert_eq!(
        decode_value(&serde_json::json!(false)),
        serde_json::json!(false)
    );
}

#[test]
fn test_decode_int() {
    assert_eq!(decode_value(&serde_json::json!(42)), serde_json::json!(42));
}

#[test]
fn test_decode_float() {
    assert_eq!(
        decode_value(&serde_json::json!(3.14)),
        serde_json::json!(3.14)
    );
}

#[test]
fn test_decode_date() {
    let v = serde_json::json!("__date:2024-01-15");
    assert_eq!(decode_value(&v), serde_json::json!("2024-01-15"));
}

#[test]
fn test_decode_localtime() {
    let v = serde_json::json!("__localtime:10:30:00");
    assert_eq!(decode_value(&v), serde_json::json!("10:30:00"));
}

#[test]
fn test_decode_localdatetime() {
    let v = serde_json::json!("__localdatetime:2024-01-15T10:30:00");
    assert_eq!(decode_value(&v), serde_json::json!("2024-01-15T10:30:00"));
}

#[test]
fn test_decode_datetime_with_tz() {
    let v = serde_json::json!("__datetime:2024-01-15T10:30:00+01:00[Europe/Berlin]");
    assert_eq!(
        decode_value(&v),
        serde_json::json!("2024-01-15T10:30:00+01:00")
    );
}

#[test]
fn test_decode_datetime_without_tz_brackets() {
    let v = serde_json::json!("__datetime:2024-01-15T10:30:00Z");
    assert_eq!(decode_value(&v), serde_json::json!("2024-01-15T10:30:00Z"));
}

#[test]
fn test_decode_duration() {
    let v = serde_json::json!("__duration:P1Y2M3D");
    assert_eq!(decode_value(&v), serde_json::json!("P1Y2M3D"));
}

#[test]
fn test_decode_point() {
    let v = serde_json::json!("__point:1.5,2.5,Wgs84");
    let decoded = decode_value(&v);
    assert_eq!(decoded["x"], serde_json::json!(1.5));
    assert_eq!(decoded["y"], serde_json::json!(2.5));
    assert_eq!(decoded["crs"], serde_json::json!("Wgs84"));
}

#[test]
fn test_decode_point_cartesian() {
    let v = serde_json::json!("__point:10.0,20.0,Cartesian");
    let decoded = decode_value(&v);
    assert_eq!(decoded["x"], serde_json::json!(10.0));
    assert_eq!(decoded["y"], serde_json::json!(20.0));
    assert_eq!(decoded["crs"], serde_json::json!("Cartesian"));
}

#[test]
fn test_decode_node() {
    let v = serde_json::json!("__node:42");
    assert_eq!(decode_value(&v), serde_json::json!(42));
}

#[test]
fn test_decode_edge() {
    let v = serde_json::json!("__edge:7");
    assert_eq!(decode_value(&v), serde_json::json!(7));
}

#[test]
fn test_decode_escaped_string() {
    let v = serde_json::json!("__str:__date:2024-01-15");
    assert_eq!(decode_value(&v), serde_json::json!("__date:2024-01-15"));
}

#[test]
fn test_decode_path() {
    let v = serde_json::json!({"__path_nodes": [1, 2, 3], "__path_edges": [10, 20]});
    let decoded = decode_value(&v);
    assert_eq!(decoded["nodes"], serde_json::json!([1, 2, 3]));
    assert_eq!(decoded["edges"], serde_json::json!([10, 20]));
}

#[test]
fn test_decode_nested_list() {
    let v = serde_json::json!(["__node:1", "__edge:2", "plain"]);
    let decoded = decode_value(&v);
    assert_eq!(decoded, serde_json::json!([1, 2, "plain"]));
}

#[test]
fn test_decode_nested_dict() {
    let v = serde_json::json!({"date": "__date:2024-01-15", "count": 5});
    let decoded = decode_value(&v);
    assert_eq!(decoded["date"], serde_json::json!("2024-01-15"));
    assert_eq!(decoded["count"], serde_json::json!(5));
}

#[test]
fn test_decode_deeply_nested() {
    let v = serde_json::json!({
        "items": [
            {"val": "__node:1"},
            {"val": "__date:2024-06-01"}
        ]
    });
    let decoded = decode_value(&v);
    let items = decoded["items"].as_array().unwrap();
    assert_eq!(items[0]["val"], serde_json::json!(1));
    assert_eq!(items[1]["val"], serde_json::json!("2024-06-01"));
}

#[test]
fn test_decode_empty_array() {
    let v = serde_json::json!([]);
    assert_eq!(decode_value(&v), serde_json::json!([]));
}

#[test]
fn test_decode_empty_object() {
    let v = serde_json::json!({});
    assert_eq!(decode_value(&v), serde_json::json!({}));
}

// ---------------------------------------------------------------------------
// 6. Wire format parsing (parse_query_response / parse_mutate_response)
// ---------------------------------------------------------------------------

#[test]
fn test_parse_query_response_basic() {
    let value = redis::Value::Array(vec![
        redis::Value::BulkString(b"{\"name\":\"Alice\",\"age\":30}".to_vec()),
        redis::Value::BulkString(b"{\"name\":\"Bob\",\"age\":25}".to_vec()),
    ]);
    let result = parse_query_response(value).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].get_str("name"), Some("Alice"));
    assert_eq!(result[0].get_i64("age"), Some(30));
    assert_eq!(result[1].get_str("name"), Some("Bob"));
}

#[test]
fn test_parse_query_response_empty() {
    let value = redis::Value::Array(vec![]);
    let result = parse_query_response(value).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_parse_query_response_nil() {
    let value = redis::Value::Nil;
    let result = parse_query_response(value).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_parse_query_response_with_tagged_values() {
    let value = redis::Value::Array(vec![redis::Value::BulkString(
        b"{\"d\":\"__date:2024-01-15\",\"n\":\"__node:42\"}".to_vec(),
    )]);
    let result = parse_query_response(value).unwrap();
    assert_eq!(result[0]["d"], serde_json::json!("2024-01-15"));
    assert_eq!(result[0]["n"], serde_json::json!(42));
}

#[test]
fn test_parse_query_response_protocol_error() {
    let value = redis::Value::Int(42);
    let err = parse_query_response(value).unwrap_err();
    match err {
        sedan_client::SedanError::Protocol(_) => {}
        other => panic!("Expected Protocol error, got {:?}", other),
    }
}

#[test]
fn test_parse_mutate_response_with_rows_and_stats() {
    let rows = redis::Value::Array(vec![redis::Value::BulkString(
        b"{\"n.name\":\"Alice\"}".to_vec(),
    )]);
    let stats = redis::Value::BulkString(
        b"nodes_created=1 nodes_deleted=0 edges_created=0 edges_deleted=0 properties_set=2 labels_added=1".to_vec(),
    );
    let value = redis::Value::Array(vec![rows, stats]);
    let result = parse_mutate_response(value).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].get_str("n.name"), Some("Alice"));
    assert_eq!(result.stats().nodes_created, 1);
    assert_eq!(result.stats().properties_set, 2);
    assert_eq!(result.stats().labels_added, 1);
}

#[test]
fn test_parse_mutate_response_empty_rows() {
    let rows = redis::Value::Array(vec![]);
    let stats = redis::Value::BulkString(
        b"nodes_created=3 nodes_deleted=0 edges_created=2 edges_deleted=0 properties_set=5 labels_added=3".to_vec(),
    );
    let value = redis::Value::Array(vec![rows, stats]);
    let result = parse_mutate_response(value).unwrap();
    assert!(result.is_empty());
    assert_eq!(result.stats().nodes_created, 3);
    assert_eq!(result.stats().edges_created, 2);
}

#[test]
fn test_parse_mutate_response_nil() {
    let value = redis::Value::Nil;
    let result = parse_mutate_response(value).unwrap();
    assert!(result.is_empty());
    assert_eq!(result.stats(), &MutationStats::default());
}

#[test]
fn test_parse_mutate_response_protocol_error() {
    let value = redis::Value::Int(42);
    let err = parse_mutate_response(value).unwrap_err();
    match err {
        sedan_client::SedanError::Protocol(_) => {}
        other => panic!("Expected Protocol error, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// 7. Error cases: malformed JSON, missing fields
// ---------------------------------------------------------------------------

#[test]
fn test_parse_query_response_malformed_json() {
    let value = redis::Value::Array(vec![redis::Value::BulkString(b"not valid json".to_vec())]);
    let err = parse_query_response(value).unwrap_err();
    match err {
        sedan_client::SedanError::Json(_) => {}
        other => panic!("Expected Json error, got {:?}", other),
    }
}

#[test]
fn test_parse_query_response_non_object_json() {
    // A valid JSON value but not a JSON object -- serde will fail to parse
    // it as a Map<String, Value>.
    let value = redis::Value::Array(vec![redis::Value::BulkString(b"[1, 2, 3]".to_vec())]);
    let err = parse_query_response(value).unwrap_err();
    match err {
        sedan_client::SedanError::Json(_) => {}
        other => panic!("Expected Json error, got {:?}", other),
    }
}

#[test]
fn test_record_get_missing_fields() {
    let r = make_record(&[("name", serde_json::json!("Alice"))]);
    assert_eq!(r.get("age"), None);
    assert_eq!(r.get_str("age"), None);
    assert_eq!(r.get_i64("age"), None);
    assert_eq!(r.get_f64("age"), None);
    assert_eq!(r.get_bool("age"), None);
}

#[test]
fn test_record_type_mismatch() {
    let r = make_record(&[("name", serde_json::json!("Alice"))]);
    assert_eq!(r.get_i64("name"), None);
    assert_eq!(r.get_bool("name"), None);
    assert_eq!(r.get_f64("name"), None);
}

// ---------------------------------------------------------------------------
// 8. SedanValue enum constructors
// ---------------------------------------------------------------------------

#[test]
fn test_sedan_value_clone() {
    let v = SedanValue::Str("test".into());
    let v2 = v.clone();
    assert_eq!(v, v2);
}

#[test]
fn test_sedan_value_list_equality() {
    let a = SedanValue::List(vec![SedanValue::Int(1), SedanValue::Int(2)]);
    let b = SedanValue::List(vec![SedanValue::Int(1), SedanValue::Int(2)]);
    assert_eq!(a, b);
}

#[test]
fn test_sedan_value_debug() {
    let v = SedanValue::Int(42);
    let debug = format!("{:?}", v);
    assert!(debug.contains("42"));
}

// ---------------------------------------------------------------------------
// 9. Point and SedanPath types
// ---------------------------------------------------------------------------

#[test]
fn test_point_construction() {
    let p = Point {
        x: 1.5,
        y: 2.5,
        crs: "Wgs84".into(),
    };
    assert_eq!(p.x, 1.5);
    assert_eq!(p.y, 2.5);
    assert_eq!(p.crs, "Wgs84");
}

#[test]
fn test_point_equality() {
    let a = Point {
        x: 1.0,
        y: 2.0,
        crs: "Wgs84".into(),
    };
    let b = Point {
        x: 1.0,
        y: 2.0,
        crs: "Wgs84".into(),
    };
    assert_eq!(a, b);
}

#[test]
fn test_sedan_path_construction() {
    let p = SedanPath {
        nodes: vec![1, 2, 3],
        edges: vec![10, 20],
    };
    assert_eq!(p.nodes, vec![1, 2, 3]);
    assert_eq!(p.edges, vec![10, 20]);
}

#[test]
fn test_sedan_path_equality() {
    let a = SedanPath {
        nodes: vec![1, 2],
        edges: vec![10],
    };
    let b = SedanPath {
        nodes: vec![1, 2],
        edges: vec![10],
    };
    assert_eq!(a, b);
}

// ---------------------------------------------------------------------------
// 10. Edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_decode_malformed_point_returns_string() {
    let v = serde_json::json!("__point:invalid");
    let decoded = decode_value(&v);
    assert_eq!(decoded, serde_json::json!("__point:invalid"));
}

#[test]
fn test_decode_node_non_numeric() {
    // __node: followed by non-numeric should return as-is
    let v = serde_json::json!("__node:abc");
    let decoded = decode_value(&v);
    assert_eq!(decoded, serde_json::json!("__node:abc"));
}

#[test]
fn test_decode_edge_non_numeric() {
    let v = serde_json::json!("__edge:xyz");
    let decoded = decode_value(&v);
    assert_eq!(decoded, serde_json::json!("__edge:xyz"));
}

#[test]
fn test_substitute_string_with_dollar_sign() {
    let result = substitute_params(
        "SET n.price = $price",
        &[("price", SedanValue::Str("$100".into()))],
    );
    assert_eq!(result, "SET n.price = '$100'");
}

#[test]
fn test_query_result_empty_stats_default() {
    let qr = QueryResult::new(vec![], MutationStats::default());
    assert_eq!(qr.stats().nodes_created, 0);
    assert_eq!(qr.stats().edges_created, 0);
}

#[test]
fn test_parse_query_response_simple_string() {
    // Some Redis responses may use SimpleString instead of BulkString
    let value = redis::Value::Array(vec![redis::Value::SimpleString(
        "{\"name\":\"Eve\"}".to_string(),
    )]);
    let result = parse_query_response(value).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].get_str("name"), Some("Eve"));
}

#[test]
fn test_parse_mutate_response_simple_string_stats() {
    let rows = redis::Value::Array(vec![]);
    let stats = redis::Value::SimpleString(
        "nodes_created=1 nodes_deleted=0 edges_created=0 edges_deleted=0 properties_set=1 labels_added=1".to_string(),
    );
    let value = redis::Value::Array(vec![rows, stats]);
    let result = parse_mutate_response(value).unwrap();
    assert_eq!(result.stats().nodes_created, 1);
    assert_eq!(result.stats().labels_added, 1);
}
