//! Conversion helpers between `IndexMap<String, Value>` and JSON object maps.
//!
//! These use the canonical tagged-string encoding defined in
//! [`crate::serde_value`] so that temporal, spatial, and list values
//! survive a serde round-trip with full fidelity.

use indexmap::IndexMap;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::serde_value::{json_to_value, value_to_json};
use crate::types::Value;

/// Convert a `IndexMap<String, Value>` to a JSON object map using the
/// canonical tagged-string encoding.  Values that fail serialisation
/// (non-finite floats) are logged and dropped — the storage layer already
/// rejects these, so this only triggers on truly corrupted data.
pub(super) fn props_to_json_map(props: &IndexMap<String, Value>) -> JsonMap<String, JsonValue> {
    let mut m = JsonMap::with_capacity(props.len());
    for (k, v) in props {
        match value_to_json(v) {
            Ok(jv) => {
                m.insert(k.clone(), jv);
            }
            Err(e) => {
                tracing::warn!(key = %k, "changelog: dropping non-serialisable property: {e}");
            }
        }
    }
    m
}

/// Convert a JSON object map back to `IndexMap<String, Value>`.
/// Values that cannot be decoded are logged and skipped.
pub fn json_map_to_props(m: &JsonMap<String, JsonValue>) -> IndexMap<String, Value> {
    m.iter()
        .filter_map(|(k, v)| match json_to_value(v) {
            Some(val) => Some((k.clone(), val)),
            None => {
                tracing::warn!(key = %k, "changelog: dropping non-decodable property");
                None
            }
        })
        .collect()
}
