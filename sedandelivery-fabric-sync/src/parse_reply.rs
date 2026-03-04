//! RESP reply parsing helpers for XREADGROUP responses.

use std::collections::HashMap;

use redis::Value as RV;

/// A stream entry: (entry_id, field->value map).
pub(crate) type StreamEntry = (String, HashMap<String, String>);

/// Extract `(entry_id, fields)` pairs from a raw XREADGROUP reply.
///
/// The Valkey/Redis RESP reply for XREADGROUP has the shape:
/// ```text
/// [stream-name, [[entry-id, [field, value, field, value, ...]], ...]]
/// ```
/// We navigate this structure defensively — any unexpected shape is logged
/// and skipped.
pub(crate) fn extract_entries(reply: RV) -> Vec<StreamEntry> {
    let mut result = Vec::new();

    // Top-level: array of [stream-key, entries]
    let streams = match reply {
        RV::Array(v) => v,
        _ => return result,
    };

    for stream in streams {
        // Each stream: [key, [[id, fields], ...]]
        let parts = match stream {
            RV::Array(v) => v,
            _ => continue,
        };
        if parts.len() < 2 {
            continue;
        }
        let entries_val = &parts[1];
        let entries = match entries_val {
            RV::Array(v) => v,
            _ => continue,
        };
        for entry in entries {
            let entry_parts = match entry {
                RV::Array(v) => v,
                _ => continue,
            };
            if entry_parts.len() < 2 {
                continue;
            }
            let id = match &entry_parts[0] {
                RV::BulkString(b) => String::from_utf8_lossy(b).into_owned(),
                RV::SimpleString(s) => s.clone(),
                _ => continue,
            };
            let fields_list = match &entry_parts[1] {
                RV::Array(v) => v,
                _ => continue,
            };
            let mut fields = HashMap::new();
            let mut i = 0;
            while i + 1 < fields_list.len() {
                let k = redis_str(&fields_list[i]);
                let v = redis_str(&fields_list[i + 1]);
                if let (Some(k), Some(v)) = (k, v) {
                    fields.insert(k, v);
                }
                i += 2;
            }
            result.push((id, fields));
        }
    }
    result
}

fn redis_str(v: &RV) -> Option<String> {
    match v {
        RV::BulkString(b) => Some(String::from_utf8_lossy(b).into_owned()),
        RV::SimpleString(s) => Some(s.clone()),
        _ => None,
    }
}
