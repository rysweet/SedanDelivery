//! Scalar `Value` → `BoltValue` dispatch and property-map helpers.

use cypher_core::types::Value;
use indexmap::IndexMap;

use crate::packstream::value::BoltValue;

use super::{spatial, temporal};

/// Convert a cypher-core property map to a `BoltValue::Map`.
///
/// Only scalar values appear in property maps (no Node/Edge/Path references).
pub fn props_to_bolt_map(props: &IndexMap<String, Value>) -> BoltValue {
    let entries: Vec<(String, BoltValue)> = props
        .iter()
        .map(|(k, v)| (k.clone(), scalar_value_to_bolt(v)))
        .collect();
    BoltValue::Map(entries)
}

/// Convert a cypher-core scalar Value to a BoltValue.
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
        Value::Date(d) => temporal::date_to_bolt(d),
        Value::LocalTime(t) => temporal::local_time_to_bolt(t),
        Value::Time(t, offset_secs) => temporal::zoned_time_to_bolt(t, *offset_secs),
        Value::LocalDateTime(dt) => temporal::local_datetime_to_bolt(dt),
        Value::DateTime(zoned) => temporal::datetime_to_bolt(zoned),
        Value::Duration(dur) => temporal::duration_to_bolt(dur),

        // ── Spatial types ────────────────────────────────────────────────
        Value::Point(p) => spatial::point_to_bolt(p),

        // Bytes: direct mapping to Bolt Bytes
        Value::Bytes(b) => BoltValue::Bytes(b.clone()),

        // Node/Edge/Path should not appear in scalar context; treat as Null
        // rather than panicking for robustness.
        Value::Node(_) | Value::Edge(_) | Value::Path { .. } => BoltValue::Null,
    }
}

fn indexmap_to_bolt_map(map: &IndexMap<Box<str>, Value>) -> BoltValue {
    let entries: Vec<(String, BoltValue)> = map
        .iter()
        .map(|(k, v)| (k.to_string(), scalar_value_to_bolt(v)))
        .collect();
    BoltValue::Map(entries)
}
