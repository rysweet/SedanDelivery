// Sanitization and Cypher-generation helpers.

use cypher_core::serde_value;
use cypher_core::storage::RangeQuery;
use cypher_core::types::Value;
use serde_json::{json, Value as JValue};

/// Sanitise a label / property name: keep alphanumeric + underscore, truncate to 128 chars.
///
/// Returns an empty string only when the input is empty. Callers MUST reject
/// empty results before interpolating into Cypher templates — an empty
/// backtick-quoted label (`` n:`` ``) is invalid syntax.
pub(crate) fn sanitize_label(s: &str) -> String {
    let result: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect();
    if result.is_empty() && !s.is_empty() {
        tracing::warn!("sanitize_label produced empty string from non-empty input: {s:?}");
    }
    if result.chars().count() < s.chars().count() {
        tracing::warn!(
            "sanitize_label truncated label from {} to {} chars",
            s.chars().count(),
            result.chars().count()
        );
    }
    result
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
    let to_json = |v: &Value| {
        serde_value::value_to_json(v).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "value_to_json failed in range_to_cypher, using null");
            JValue::Null
        })
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── sanitize_label ───────────────────────────────────────────────────────

    #[test]
    fn sanitize_label_alphanumeric_passthrough() {
        assert_eq!(sanitize_label("Person"), "Person");
        assert_eq!(sanitize_label("my_label_42"), "my_label_42");
    }

    #[test]
    fn sanitize_label_replaces_special_chars() {
        assert_eq!(sanitize_label("my-label"), "my_label");
        assert_eq!(sanitize_label("has space"), "has_space");
        assert_eq!(sanitize_label("dot.sep"), "dot_sep");
        assert_eq!(sanitize_label("path/to"), "path_to");
    }

    #[test]
    fn sanitize_label_sql_injection_neutralized() {
        assert_eq!(sanitize_label("'; DROP TABLE--"), "___DROP_TABLE__");
        assert_eq!(sanitize_label("label`; MATCH"), "label___MATCH");
        assert_eq!(sanitize_label("a); RETURN 1"), "a___RETURN_1");
    }

    #[test]
    fn sanitize_label_empty_string() {
        assert_eq!(sanitize_label(""), "");
    }

    #[test]
    fn sanitize_label_truncates_to_128() {
        let long = "a".repeat(200);
        let result = sanitize_label(&long);
        assert_eq!(result.len(), 128);
        assert!(result.chars().all(|c| c == 'a'));
    }

    #[test]
    fn sanitize_label_unicode_alphanumeric_preserved() {
        let result = sanitize_label("\u{4eba}");
        assert_eq!(result, "\u{4eba}");
    }

    #[test]
    fn sanitize_label_unicode_non_alnum_replaced() {
        assert_eq!(sanitize_label("lab\u{2603}el"), "lab_el");
    }

    // ── urlenc ───────────────────────────────────────────────────────────────

    #[test]
    fn urlenc_unreserved_passthrough() {
        assert_eq!(urlenc("abc123"), "abc123");
        assert_eq!(urlenc("A-Z_a.z~0"), "A-Z_a.z~0");
    }

    #[test]
    fn urlenc_encodes_spaces_and_special() {
        assert_eq!(urlenc("hello world"), "hello%20world");
        assert_eq!(urlenc("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn urlenc_encodes_multibyte_utf8() {
        assert_eq!(urlenc("\u{00e9}"), "%C3%A9");
    }

    #[test]
    fn urlenc_empty_string() {
        assert_eq!(urlenc(""), "");
    }

    #[test]
    fn urlenc_all_reserved_chars() {
        assert_eq!(urlenc(":"), "%3A");
        assert_eq!(urlenc("/"), "%2F");
        assert_eq!(urlenc("?"), "%3F");
        assert_eq!(urlenc("#"), "%23");
        assert_eq!(urlenc("@"), "%40");
        assert_eq!(urlenc("+"), "%2B");
    }

    // ── range_to_cypher ──────────────────────────────────────────────────────

    #[test]
    fn range_eq() {
        let v = Value::Int(42);
        let (pred, params) = range_to_cypher("age", RangeQuery::Eq(&v));
        assert_eq!(pred, "n.`age` = $rv");
        assert_eq!(params["rv"], 42);
    }

    #[test]
    fn range_lt() {
        let v = Value::Int(10);
        let (pred, _params) = range_to_cypher("x", RangeQuery::Lt(&v));
        assert_eq!(pred, "n.`x` < $rv");
    }

    #[test]
    fn range_lte() {
        let v = Value::Float(std::f64::consts::PI);
        let (pred, params) = range_to_cypher("val", RangeQuery::Lte(&v));
        assert_eq!(pred, "n.`val` <= $rv");
        assert!((params["rv"].as_f64().unwrap() - std::f64::consts::PI).abs() < f64::EPSILON);
    }

    #[test]
    fn range_gt() {
        let v = Value::Int(0);
        let (pred, _) = range_to_cypher("score", RangeQuery::Gt(&v));
        assert_eq!(pred, "n.`score` > $rv");
    }

    #[test]
    fn range_gte() {
        let v = Value::Int(100);
        let (pred, _) = range_to_cypher("level", RangeQuery::Gte(&v));
        assert_eq!(pred, "n.`level` >= $rv");
    }

    #[test]
    fn range_between_inclusive() {
        let lo = Value::Int(1);
        let hi = Value::Int(10);
        let (pred, params) = range_to_cypher(
            "rank",
            RangeQuery::Between {
                lo: &lo,
                hi: &hi,
                inclusive: true,
            },
        );
        assert_eq!(pred, "n.`rank` >= $lo AND n.`rank` <= $hi");
        assert_eq!(params["lo"], 1);
        assert_eq!(params["hi"], 10);
    }

    #[test]
    fn range_between_exclusive() {
        let lo = Value::Int(0);
        let hi = Value::Int(99);
        let (pred, params) = range_to_cypher(
            "idx",
            RangeQuery::Between {
                lo: &lo,
                hi: &hi,
                inclusive: false,
            },
        );
        assert_eq!(pred, "n.`idx` > $lo AND n.`idx` < $hi");
        assert_eq!(params["lo"], 0);
        assert_eq!(params["hi"], 99);
    }

    #[test]
    fn range_starts_with() {
        let (pred, params) = range_to_cypher("name", RangeQuery::StartsWith("Al"));
        assert_eq!(pred, "n.`name` STARTS WITH $prefix");
        assert_eq!(params["prefix"], "Al");
    }

    #[test]
    fn range_non_finite_float_becomes_null() {
        let v = Value::Float(f64::NAN);
        let (_pred, params) = range_to_cypher("x", RangeQuery::Eq(&v));
        assert!(params["rv"].is_null());
    }
}
