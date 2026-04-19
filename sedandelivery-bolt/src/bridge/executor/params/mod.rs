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
mod tests_e2e;
#[cfg(test)]
mod tests_roundtrip;
#[cfg(test)]
mod tests_scalar;
#[cfg(test)]
mod tests_structure;
