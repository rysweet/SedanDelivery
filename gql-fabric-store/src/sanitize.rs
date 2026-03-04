// Sanitization and Cypher-generation helpers.

use gql_core::serde_value;
use gql_core::storage::RangeQuery;
use gql_core::types::Value;
use serde_json::{json, Value as JValue};

/// Sanitise a label / property name: keep alphanumeric + underscore, truncate to 128 chars.
pub(crate) fn sanitize_label(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect()
}

/// URL-encode a string for use in x-www-form-urlencoded bodies.
/// Encodes individual UTF-8 bytes rather than Unicode codepoints, which is the
/// correct behavior per RFC 3986 for multi-byte characters.
pub(crate) fn urlenc(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len() * 3);
    for &byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b => write!(out, "%{b:02X}").unwrap(),
        }
    }
    out
}

/// Build a Cypher WHERE predicate string and parameter map for a RangeQuery.
/// Non-finite floats in range values are serialized as JSON null.
pub(crate) fn range_to_cypher(prop: &str, range: RangeQuery<'_>) -> (String, JValue) {
    let to_json = |v: &Value| serde_value::value_to_json(v).unwrap_or(JValue::Null);
    match range {
        RangeQuery::Eq(v) => (format!("n.`{prop}` = $rv"), json!({ "rv": to_json(v) })),
        RangeQuery::Lt(v) => (format!("n.`{prop}` < $rv"), json!({ "rv": to_json(v) })),
        RangeQuery::Lte(v) => (format!("n.`{prop}` <= $rv"), json!({ "rv": to_json(v) })),
        RangeQuery::Gt(v) => (format!("n.`{prop}` > $rv"), json!({ "rv": to_json(v) })),
        RangeQuery::Gte(v) => (format!("n.`{prop}` >= $rv"), json!({ "rv": to_json(v) })),
        RangeQuery::Between { lo, hi, inclusive } => {
            if inclusive {
                (
                    format!("n.`{prop}` >= $lo AND n.`{prop}` <= $hi"),
                    json!({ "lo": to_json(lo), "hi": to_json(hi) }),
                )
            } else {
                (
                    format!("n.`{prop}` > $lo AND n.`{prop}` < $hi"),
                    json!({ "lo": to_json(lo), "hi": to_json(hi) }),
                )
            }
        }
        RangeQuery::StartsWith(prefix) => (
            format!("n.`{prop}` STARTS WITH $prefix"),
            json!({ "prefix": prefix }),
        ),
    }
}
