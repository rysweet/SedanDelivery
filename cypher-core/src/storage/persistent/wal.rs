//! WAL entry definitions and JSON-safe value conversion.
//!
//! Each WAL line is a serialized [`WalEntry`]. Property values use the
//! [`JsonValue`] wrapper so that the on-disk format is independent of the
//! richer [`Value`] enum used in memory.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::types::Value;

/// A single mutation recorded in the WAL or snapshot file. One JSON line per entry.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "op")]
pub(super) enum WalEntry {
    CreateGraph {
        name: String,
    },
    DeleteGraph {
        name: String,
    },
    CreateNode {
        graph: String,
        labels: Vec<String>,
        properties: Vec<(String, JsonValue)>,
        id: u64,
    },
    CreateEdge {
        graph: String,
        src: u64,
        dst: u64,
        rel_type: String,
        properties: Vec<(String, JsonValue)>,
        id: u64,
    },
    SetNodeProperties {
        graph: String,
        id: u64,
        properties: Vec<(String, JsonValue)>,
    },
    ReplaceNodeProperties {
        graph: String,
        id: u64,
        properties: Vec<(String, JsonValue)>,
    },
    SetEdgeProperties {
        graph: String,
        id: u64,
        properties: Vec<(String, JsonValue)>,
    },
    ReplaceEdgeProperties {
        graph: String,
        id: u64,
        properties: Vec<(String, JsonValue)>,
    },
    DeleteNode {
        graph: String,
        id: u64,
    },
    DeleteEdge {
        graph: String,
        id: u64,
    },
    AddNodeLabels {
        graph: String,
        id: u64,
        labels: Vec<String>,
    },
    RemoveNodeLabels {
        graph: String,
        id: u64,
        labels: Vec<String>,
    },
    RemoveNodeProperty {
        graph: String,
        id: u64,
        key: String,
    },
    RemoveEdgeProperty {
        graph: String,
        id: u64,
        key: String,
    },
    CreateIndex {
        graph: String,
        label: String,
        property: String,
    },
    DropIndex {
        graph: String,
        label: String,
        property: String,
    },
    CreateConstraint {
        graph: String,
        label: String,
        property: String,
        constraint_type: crate::storage::ConstraintType,
    },
    DropConstraint {
        graph: String,
        label: String,
        property: String,
    },
}

/// A JSON-safe representation of [`Value`]. We only persist property values, which are
/// the subset of Value that can appear as node/edge properties (no Node/Edge/Path refs).
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "t", content = "v")]
pub(super) enum JsonValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<JsonValue>),
    Map(Vec<(String, JsonValue)>),
    Bytes(Vec<u8>),
}

pub(super) fn value_to_json(v: &Value) -> JsonValue {
    match v {
        Value::Null => JsonValue::Null,
        Value::Bool(b) => JsonValue::Bool(*b),
        Value::Int(i) => JsonValue::Int(*i),
        Value::Float(f) => JsonValue::Float(*f),
        Value::Str(s) => JsonValue::Str(s.to_string()),
        Value::List(items) => JsonValue::List(items.iter().map(value_to_json).collect()),
        Value::Map(m) => JsonValue::Map(
            m.iter()
                .map(|(k, v)| (k.to_string(), value_to_json(v)))
                .collect(),
        ),
        Value::Bytes(b) => JsonValue::Bytes(b.clone()),
        // Temporal and spatial types are not commonly used as properties in the WAL path.
        // Store them as their string representation for durability; they will be restored
        // as strings on replay.
        other => JsonValue::Str(format!("{:?}", other)),
    }
}

pub(super) fn json_to_value(jv: JsonValue) -> Value {
    match jv {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(b) => Value::Bool(b),
        JsonValue::Int(i) => Value::Int(i),
        JsonValue::Float(f) => Value::Float(f),
        JsonValue::Str(s) => Value::Str(s.into_boxed_str()),
        JsonValue::List(items) => Value::List(items.into_iter().map(json_to_value).collect()),
        JsonValue::Map(m) => Value::Map(
            m.into_iter()
                .map(|(k, v)| (k.into_boxed_str(), json_to_value(v)))
                .collect(),
        ),
        JsonValue::Bytes(b) => Value::Bytes(b),
    }
}

pub(super) fn props_to_json(props: &IndexMap<String, Value>) -> Vec<(String, JsonValue)> {
    props
        .iter()
        .map(|(k, v)| (k.clone(), value_to_json(v)))
        .collect()
}

pub(super) fn json_to_props(entries: Vec<(String, JsonValue)>) -> IndexMap<String, Value> {
    entries
        .into_iter()
        .map(|(k, v)| (k, json_to_value(v)))
        .collect()
}

// ── Per-record framing (issue #1702) ─────────────────────────────────────────
//
// Format: each WAL line is prefixed with `CRC:<hex8> ` (11 bytes total) where
// `<hex8>` is the lower-case hex encoding of the CRC32C (Castagnoli) checksum
// of the JSON payload bytes that follow the single space separator.
//
// Example:
//     CRC:a1b2c3d4 {"op":"CreateNode",...}\n
//
// Backward compatibility: a line that does not start with `CRC:<hex8> ` is
// treated as a legacy (v1) unchecksummed record and replayed unverified.
// Recovery warns once per replay when such a line is seen so operators can
// rotate to the new format.

/// 11 bytes: `CRC:` (4) + 8 hex + space (1) = 13. Wait — recompute:
/// `CRC:` = 4 bytes, 8 hex digits = 8 bytes, 1 space = 1 byte → 13 bytes total.
pub(super) const CRC_PREFIX_LEN: usize = 13;

/// Compute the CRC32C checksum of `payload` and return the framed WAL line
/// (without a trailing newline).
pub(super) fn frame_wal_line(payload: &str) -> String {
    let crc = crc32c::crc32c(payload.as_bytes());
    format!("CRC:{:08x} {}", crc, payload)
}

/// Outcome of inspecting a single WAL line.
pub(super) enum WalFrame<'a> {
    /// v2 framed record — checksum verified against the payload.
    Checked(&'a str),
    /// v2 framed record — prefix parsed but checksum did not match.
    Corrupt { expected: u32, actual: u32 },
    /// Legacy (v1) record — no `CRC:` prefix, returned verbatim.
    Legacy(&'a str),
}

/// Inspect a WAL line (no trailing newline). Distinguishes v2 framed records
/// from legacy unchecksummed records and verifies the CRC when present.
pub(super) fn inspect_wal_line(line: &str) -> WalFrame<'_> {
    // Must start with `CRC:` and have at least 8 hex + space after.
    if !line.is_char_boundary(CRC_PREFIX_LEN) || !line.starts_with("CRC:") {
        return WalFrame::Legacy(line);
    }
    let hex = &line[4..12];
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) || line.as_bytes()[12] != b' ' {
        return WalFrame::Legacy(line);
    }
    let expected = match u32::from_str_radix(hex, 16) {
        Ok(v) => v,
        Err(_) => return WalFrame::Legacy(line),
    };
    let payload = &line[CRC_PREFIX_LEN..];
    let actual = crc32c::crc32c(payload.as_bytes());
    if actual == expected {
        WalFrame::Checked(payload)
    } else {
        WalFrame::Corrupt { expected, actual }
    }
}
