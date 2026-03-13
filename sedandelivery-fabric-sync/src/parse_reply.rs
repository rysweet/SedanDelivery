//! RESP reply parsing helpers for XREADGROUP responses.

use std::collections::HashMap;

use redis::Value as RV;

/// A stream entry: (stream_key, entry_id, field->value map).
pub(crate) type StreamEntry = (String, String, HashMap<String, String>);

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
        let stream_key = match &parts[0] {
            RV::BulkString(b) => String::from_utf8_lossy(b).into_owned(),
            RV::SimpleString(s) => s.clone(),
            _ => continue,
        };
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
            result.push((stream_key.clone(), id, fields));
        }
    }
    result
}

pub(crate) fn redis_str(v: &RV) -> Option<String> {
    match v {
        RV::BulkString(b) => Some(String::from_utf8_lossy(b).into_owned()),
        RV::SimpleString(s) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── redis_str ────────────────────────────────────────────────────────────

    #[test]
    fn redis_str_bulk_string() {
        let v = RV::BulkString(b"hello".to_vec());
        assert_eq!(redis_str(&v), Some("hello".to_string()));
    }

    #[test]
    fn redis_str_simple_string() {
        let v = RV::SimpleString("OK".into());
        assert_eq!(redis_str(&v), Some("OK".to_string()));
    }

    #[test]
    fn redis_str_integer_returns_none() {
        let v = RV::Int(42);
        assert_eq!(redis_str(&v), None);
    }

    #[test]
    fn redis_str_nil_returns_none() {
        let v = RV::Nil;
        assert_eq!(redis_str(&v), None);
    }

    #[test]
    fn redis_str_non_utf8_lossy() {
        let v = RV::BulkString(vec![0xFF, 0xFE]);
        let result = redis_str(&v);
        assert!(result.is_some());
        assert!(result.unwrap().contains('\u{FFFD}'));
    }

    // ── extract_entries ─────────────────────────────────────────────────────

    #[test]
    fn extract_entries_valid_single_stream() {
        let reply = RV::Array(vec![RV::Array(vec![
            RV::BulkString(b"mystream".to_vec()),
            RV::Array(vec![RV::Array(vec![
                RV::BulkString(b"1-0".to_vec()),
                RV::Array(vec![
                    RV::BulkString(b"e".to_vec()),
                    RV::BulkString(b"{\"CreateGraph\":{\"name\":\"g1\"}}".to_vec()),
                ]),
            ])]),
        ])]);
        let entries = extract_entries(reply);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "mystream");
        assert_eq!(entries[0].1, "1-0");
        assert_eq!(
            entries[0].2.get("e").unwrap(),
            "{\"CreateGraph\":{\"name\":\"g1\"}}"
        );
    }

    #[test]
    fn extract_entries_multiple_entries() {
        let reply = RV::Array(vec![RV::Array(vec![
            RV::BulkString(b"s".to_vec()),
            RV::Array(vec![
                RV::Array(vec![
                    RV::BulkString(b"1-0".to_vec()),
                    RV::Array(vec![
                        RV::BulkString(b"k1".to_vec()),
                        RV::BulkString(b"v1".to_vec()),
                    ]),
                ]),
                RV::Array(vec![
                    RV::BulkString(b"2-0".to_vec()),
                    RV::Array(vec![
                        RV::BulkString(b"k2".to_vec()),
                        RV::BulkString(b"v2".to_vec()),
                    ]),
                ]),
            ]),
        ])]);
        let entries = extract_entries(reply);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "s");
        assert_eq!(entries[0].1, "1-0");
        assert_eq!(entries[1].0, "s");
        assert_eq!(entries[1].1, "2-0");
    }

    #[test]
    fn extract_entries_non_array_returns_empty() {
        assert!(extract_entries(RV::Nil).is_empty());
        assert!(extract_entries(RV::Int(0)).is_empty());
        assert!(extract_entries(RV::BulkString(b"nope".to_vec())).is_empty());
    }

    #[test]
    fn extract_entries_empty_array() {
        assert!(extract_entries(RV::Array(vec![])).is_empty());
    }

    #[test]
    fn extract_entries_stream_too_few_parts() {
        let reply = RV::Array(vec![RV::Array(vec![RV::BulkString(b"key".to_vec())])]);
        assert!(extract_entries(reply).is_empty());
    }

    #[test]
    fn extract_entries_skips_non_array_entry() {
        let reply = RV::Array(vec![RV::Array(vec![
            RV::BulkString(b"s".to_vec()),
            RV::Array(vec![RV::Int(42)]),
        ])]);
        assert!(extract_entries(reply).is_empty());
    }

    #[test]
    fn extract_entries_entry_too_few_parts() {
        let reply = RV::Array(vec![RV::Array(vec![
            RV::BulkString(b"s".to_vec()),
            RV::Array(vec![RV::Array(vec![RV::BulkString(b"1-0".to_vec())])]),
        ])]);
        assert!(extract_entries(reply).is_empty());
    }

    #[test]
    fn extract_entries_simple_string_entry_id() {
        let reply = RV::Array(vec![RV::Array(vec![
            RV::BulkString(b"s".to_vec()),
            RV::Array(vec![RV::Array(vec![
                RV::SimpleString("3-0".into()),
                RV::Array(vec![
                    RV::BulkString(b"f".to_vec()),
                    RV::BulkString(b"v".to_vec()),
                ]),
            ])]),
        ])]);
        let entries = extract_entries(reply);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1, "3-0");
    }

    #[test]
    fn extract_entries_non_string_field_key_skipped() {
        let reply = RV::Array(vec![RV::Array(vec![
            RV::BulkString(b"s".to_vec()),
            RV::Array(vec![RV::Array(vec![
                RV::BulkString(b"1-0".to_vec()),
                RV::Array(vec![
                    RV::Int(99),
                    RV::BulkString(b"val".to_vec()),
                    RV::BulkString(b"good".to_vec()),
                    RV::BulkString(b"yes".to_vec()),
                ]),
            ])]),
        ])]);
        let entries = extract_entries(reply);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].2.len(), 1);
        assert_eq!(entries[0].2.get("good").unwrap(), "yes");
    }

    #[test]
    fn extract_entries_odd_field_count_ignores_trailing() {
        let reply = RV::Array(vec![RV::Array(vec![
            RV::BulkString(b"s".to_vec()),
            RV::Array(vec![RV::Array(vec![
                RV::BulkString(b"1-0".to_vec()),
                RV::Array(vec![
                    RV::BulkString(b"k".to_vec()),
                    RV::BulkString(b"v".to_vec()),
                    RV::BulkString(b"orphan".to_vec()),
                ]),
            ])]),
        ])]);
        let entries = extract_entries(reply);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].2.len(), 1);
        assert_eq!(entries[0].2.get("k").unwrap(), "v");
    }
}
