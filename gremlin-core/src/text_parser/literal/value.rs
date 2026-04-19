//! Single-value parsing: `parse_literal_value` and the rich `parse_single_value`.

use super::super::tokenizer::*;
use super::super::*;
use super::collection::parse_arg_list;
use super::string::unquote;

pub(crate) fn parse_literal_value(
    s: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Option<GremlinValue> {
    // Integer with type suffix: 0b (byte), 0s (short), 0i (int), 0L/0l (long), 0n (BigInteger)
    if let Some(stripped) = s
        .strip_suffix('b')
        .or_else(|| s.strip_suffix('B'))
        .or_else(|| s.strip_suffix('s'))
        .or_else(|| s.strip_suffix('S'))
        .or_else(|| s.strip_suffix('i'))
        .or_else(|| s.strip_suffix('I'))
        .or_else(|| s.strip_suffix('l'))
        .or_else(|| s.strip_suffix('L'))
        .or_else(|| s.strip_suffix('n'))
        .or_else(|| s.strip_suffix('N'))
    {
        if let Ok(v) = stripped.parse::<i64>() {
            return Some(GremlinValue::Integer(v));
        }
        // BigInteger values too large for i64: parse as f64 for approximate arithmetic
        if let Ok(v) = stripped.parse::<f64>() {
            return Some(GremlinValue::Float(v));
        }
    }
    if let Some(stripped) = s
        .strip_suffix('d')
        .or_else(|| s.strip_suffix('D'))
        .or_else(|| s.strip_suffix('f'))
        .or_else(|| s.strip_suffix('F'))
        .or_else(|| s.strip_suffix('m'))
        .or_else(|| s.strip_suffix('M'))
    {
        if let Ok(v) = stripped.parse::<f64>() {
            return Some(GremlinValue::Float(v));
        }
    }
    if let Ok(v) = s.parse::<i64>() {
        return Some(GremlinValue::Integer(v));
    }
    if let Ok(v) = s.parse::<f64>() {
        return Some(GremlinValue::Float(v));
    }
    let unquoted = s.trim_matches('"').trim_matches('\'');
    if unquoted != s {
        return Some(GremlinValue::String(unquoted.to_string()));
    }
    // Boolean literals
    if s == "true" {
        return Some(GremlinValue::Bool(true));
    }
    if s == "false" {
        return Some(GremlinValue::Bool(false));
    }
    // Null literal
    if s == "null" {
        return Some(GremlinValue::Null);
    }
    resolve_param(s).or(None)
}

/// Parse a single value: string literal, number, boolean, null, or parameter reference.
pub(crate) fn parse_single_value(
    s: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<GremlinValue, TextParseError> {
    let s = s.trim();

    if s.is_empty() {
        return Ok(GremlinValue::Null);
    }

    // String literal (double quotes)
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        let unescaped = inner.replace("\\\"", "\"").replace("\\\\", "\\");
        return Ok(GremlinValue::String(unescaped));
    }

    // String literal (single quotes)
    if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        let unescaped = inner.replace("\\'", "'").replace("\\\\", "\\");
        return Ok(GremlinValue::String(unescaped));
    }

    // Boolean
    if s == "true" {
        return Ok(GremlinValue::Bool(true));
    }
    if s == "false" {
        return Ok(GremlinValue::Bool(false));
    }

    // Null
    if s == "null" {
        return Ok(GremlinValue::Null);
    }

    // NaN / Infinity
    if s == "NaN" || s == "Double.NaN" || s == "Float.NaN" {
        return Ok(GremlinValue::Float(f64::NAN));
    }
    if s == "Infinity" || s == "+Infinity" || s == "Double.POSITIVE_INFINITY" {
        return Ok(GremlinValue::Float(f64::INFINITY));
    }
    if s == "-Infinity" || s == "Double.NEGATIVE_INFINITY" {
        return Ok(GremlinValue::Float(f64::NEG_INFINITY));
    }

    // Set literal: {1, 2, 3} — preserve set-ness via an internal tagged map.
    if s.starts_with('{') && s.ends_with('}') {
        let inner = &s[1..s.len() - 1];
        let items = parse_arg_list(inner, resolve_param)?;
        let mut deduped: Vec<GremlinValue> = Vec::new();
        for item in items {
            if !deduped.contains(&item) {
                deduped.push(item);
            }
        }
        let mut tagged = IndexMap::new();
        tagged.insert("~set".to_string(), GremlinValue::List(deduped));
        return Ok(GremlinValue::Map(tagged));
    }

    // List or Map literal: [1, 2, "a"] or ["key": "val", ...]
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        // Detect Groovy map literal: ["key": "val", ...] — check for top-level ':'
        let mut depth = 0u32;
        let mut in_str = false;
        let mut escape = false;
        let mut has_colon = false;
        for ch in inner.chars() {
            if escape {
                escape = false;
                continue;
            }
            if ch == '\\' && in_str {
                escape = true;
                continue;
            }
            if ch == '"' || ch == '\'' {
                in_str = !in_str;
                continue;
            }
            if in_str {
                continue;
            }
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                ':' if depth == 0 => {
                    has_colon = true;
                    break;
                }
                _ => {}
            }
        }
        if has_colon {
            // Empty map: [:]
            if inner.trim() == ":" {
                return Ok(GremlinValue::Map(IndexMap::new()));
            }
            // Parse as map literal
            let parts = split_top_level_args(inner);
            let mut map = IndexMap::new();
            for part in &parts {
                let part = part.trim();
                // Find the first top-level ':'
                let mut d = 0u32;
                let mut in_s = false;
                let mut esc = false;
                let mut colon_pos = None;
                for (i, ch) in part.char_indices() {
                    if esc {
                        esc = false;
                        continue;
                    }
                    if ch == '\\' && in_s {
                        esc = true;
                        continue;
                    }
                    if ch == '"' || ch == '\'' {
                        in_s = !in_s;
                        continue;
                    }
                    if in_s {
                        continue;
                    }
                    match ch {
                        '(' | '[' | '{' => d += 1,
                        ')' | ']' | '}' => d = d.saturating_sub(1),
                        ':' if d == 0 => {
                            colon_pos = Some(i);
                            break;
                        }
                        _ => {}
                    }
                }
                if let Some(cp) = colon_pos {
                    let key_str = part[..cp].trim();
                    let val_str = part[cp + 1..].trim();
                    let key = unquote(key_str);
                    let val = parse_single_value(val_str, resolve_param)?;
                    map.insert(key, val);
                }
            }
            return Ok(GremlinValue::Map(map));
        }
        let items = parse_arg_list(inner, resolve_param)?;
        return Ok(GremlinValue::List(items));
    }

    // Integer
    if let Ok(n) = s.parse::<i64>() {
        return Ok(GremlinValue::Integer(n));
    }

    // Float
    if let Ok(f) = s.parse::<f64>() {
        return Ok(GremlinValue::Float(f));
    }

    // Numeric with type suffix: 1b (byte), 2s (short), 3i (int), 4L (long), 0.5f/0.5d (float/double)
    // Use char-boundary-safe slicing — `s.len() - 1` would split a multi-byte
    // UTF-8 character and panic. The suffix is always a single ASCII letter,
    // so a non-ASCII final character means there is no suffix to strip.
    if let Some(last_ch) = s.chars().next_back() {
        if last_ch.is_ascii() && s.len() > 1 {
            let suffix = last_ch as u8;
            let num_part = &s[..s.len() - 1];
            if matches!(
                suffix,
                b'b' | b's' | b'i' | b'l' | b'L' | b'n' | b'B' | b'S' | b'I' | b'N'
            ) {
                if let Ok(n) = num_part.parse::<i64>() {
                    return Ok(GremlinValue::Integer(n));
                }
            }
            if matches!(suffix, b'f' | b'd' | b'F' | b'D' | b'm' | b'M') {
                if let Ok(f) = num_part.parse::<f64>() {
                    return Ok(GremlinValue::Float(f));
                }
            }
        }
    }

    // Typed value notation from feature files: d[29].i, d[0.5].d, d[6].l
    if s.starts_with("d[") {
        if let Some(end) = s.find(']') {
            let num_str = &s[2..end];
            if let Ok(n) = num_str.parse::<i64>() {
                return Ok(GremlinValue::Integer(n));
            }
            if let Ok(f) = num_str.parse::<f64>() {
                return Ok(GremlinValue::Float(f));
            }
        }
    }

    // T.label, T.id — special tokens
    if s == "T.label" || s == "t[label]" {
        return Ok(GremlinValue::String("~label".to_string()));
    }
    if s == "T.id" || s == "t[id]" {
        return Ok(GremlinValue::String("~id".to_string()));
    }

    // Scope.local, Scope.global
    if s.starts_with("Scope.") || s.starts_with("Column.") || s.starts_with("Order.") {
        return Ok(GremlinValue::String(s.to_string()));
    }

    // Merge.* — merge step option tokens
    if s.starts_with("Merge.") {
        return Ok(GremlinValue::String(s.to_string()));
    }

    // Pick.none — default branch marker for choose/option
    if s == "Pick.none" {
        return Ok(GremlinValue::String("none".to_string()));
    }

    // GType.* — Gremlin type constants
    if let Some(gtype) = s.strip_prefix("GType.") {
        return Ok(GremlinValue::String(format!("GType.{gtype}")));
    }

    // datetime("2023-01-01T00:00:00Z") — ISO datetime literal
    if let Some(rest) = s.strip_prefix("datetime(") {
        if let Some(inner) = rest.strip_suffix(')') {
            let iso_str = inner.trim().trim_matches('"').trim_matches('\'');
            return Ok(GremlinValue::String(format!("__gremlin_dt__:{iso_str}")));
        }
    }

    // UUID("5100808b-...") — UUID literal
    if let Some(rest) = s.strip_prefix("UUID(") {
        if let Some(inner) = rest.strip_suffix(')') {
            let uuid_str = inner.trim().trim_matches('"').trim_matches('\'');
            return Ok(GremlinValue::String(format!("__gremlin_uuid__:{uuid_str}")));
        }
    }

    // Parameter reference (bare identifier like vid1, xx1)
    if let Some(val) = resolve_param(s) {
        return Ok(val);
    }

    // __.select("a") or similar traversal fragment — treat as string
    if s.starts_with("__") {
        return Ok(GremlinValue::String(s.to_string()));
    }

    // Unknown — treat as parameter name / string
    Ok(GremlinValue::String(s.to_string()))
}

#[cfg(test)]
mod utf8_panic_tests {
    use crate::text_parser::parse_traversal;

    #[test]
    fn parse_traversal_does_not_panic_on_multibyte_input() {
        // Regression test for issue #1674: this fuzz input previously
        // panicked inside `parse_single_value` when slicing
        // `&s[..s.len()-1]` cut through a multi-byte UTF-8 character.
        let input = "g.V(nows\").values(\"n\"n(\"\"ӊѐ)";
        let _ = parse_traversal(input, &|_| None);
    }

    #[test]
    fn parse_traversal_handles_unicode_string_args() {
        let _ = parse_traversal("g.V().has(\"name\", \"日本語\")", &|_| None);
    }
}

#[cfg(test)]
mod direct_api_tests {
    use super::parse_single_value;
    use crate::types::GremlinValue;

    #[test]
    fn byte_lower() {
        assert_eq!(
            parse_single_value("1b", &|_| None).unwrap(),
            GremlinValue::Integer(1)
        );
    }
    #[test]
    fn byte_upper() {
        assert_eq!(
            parse_single_value("1B", &|_| None).unwrap(),
            GremlinValue::Integer(1)
        );
    }
    #[test]
    fn short_lower() {
        assert_eq!(
            parse_single_value("1s", &|_| None).unwrap(),
            GremlinValue::Integer(1)
        );
    }
    #[test]
    fn int_lower() {
        assert_eq!(
            parse_single_value("1i", &|_| None).unwrap(),
            GremlinValue::Integer(1)
        );
    }
    #[test]
    fn long_lower() {
        assert_eq!(
            parse_single_value("1l", &|_| None).unwrap(),
            GremlinValue::Integer(1)
        );
    }
    #[test]
    fn long_upper() {
        assert_eq!(
            parse_single_value("1L", &|_| None).unwrap(),
            GremlinValue::Integer(1)
        );
    }
    #[test]
    fn double_suffix() {
        assert_eq!(
            parse_single_value("0.5d", &|_| None).unwrap(),
            GremlinValue::Float(0.5)
        );
    }
    #[test]
    fn float_suffix() {
        assert_eq!(
            parse_single_value("0.5f", &|_| None).unwrap(),
            GremlinValue::Float(0.5)
        );
    }
    #[test]
    fn decimal_suffix() {
        assert_eq!(
            parse_single_value("0.5m", &|_| None).unwrap(),
            GremlinValue::Float(0.5)
        );
    }
    #[test]
    fn string_double() {
        assert_eq!(
            parse_single_value("\"abc\"", &|_| None).unwrap(),
            GremlinValue::String("abc".to_string())
        );
    }
    #[test]
    fn string_single() {
        assert_eq!(
            parse_single_value("'abc'", &|_| None).unwrap(),
            GremlinValue::String("abc".to_string())
        );
    }
    #[test]
    fn string_double_escape() {
        assert_eq!(
            parse_single_value("\"a\\\"b\"", &|_| None).unwrap(),
            GremlinValue::String("a\"b".to_string())
        );
    }
    #[test]
    fn string_single_escape() {
        assert_eq!(
            parse_single_value("'a\\'b'", &|_| None).unwrap(),
            GremlinValue::String("a'b".to_string())
        );
    }
    #[test]
    fn bool_true() {
        assert_eq!(
            parse_single_value("true", &|_| None).unwrap(),
            GremlinValue::Bool(true)
        );
    }
    #[test]
    fn bool_false() {
        assert_eq!(
            parse_single_value("false", &|_| None).unwrap(),
            GremlinValue::Bool(false)
        );
    }
    #[test]
    fn null_literal() {
        assert_eq!(
            parse_single_value("null", &|_| None).unwrap(),
            GremlinValue::Null
        );
    }
    #[test]
    fn nan_literal() {
        if let GremlinValue::Float(f) = parse_single_value("NaN", &|_| None).unwrap() {
            assert!(f.is_nan());
        } else {
            panic!("expected NaN");
        }
    }
    #[test]
    fn pos_infinity() {
        assert_eq!(
            parse_single_value("Infinity", &|_| None).unwrap(),
            GremlinValue::Float(f64::INFINITY)
        );
    }
    #[test]
    fn neg_infinity() {
        assert_eq!(
            parse_single_value("-Infinity", &|_| None).unwrap(),
            GremlinValue::Float(f64::NEG_INFINITY)
        );
    }
    #[test]
    fn double_pos_infinity() {
        assert_eq!(
            parse_single_value("Double.POSITIVE_INFINITY", &|_| None).unwrap(),
            GremlinValue::Float(f64::INFINITY)
        );
    }
    #[test]
    fn double_neg_infinity() {
        assert_eq!(
            parse_single_value("Double.NEGATIVE_INFINITY", &|_| None).unwrap(),
            GremlinValue::Float(f64::NEG_INFINITY)
        );
    }
    #[test]
    fn set_literal_dedups() {
        if let GremlinValue::Map(m) = parse_single_value("{1, 2, 2, 3}", &|_| None).unwrap() {
            if let Some(GremlinValue::List(l)) = m.get("~set") {
                assert_eq!(l.len(), 3);
            } else {
                panic!("expected ~set list");
            }
        } else {
            panic!("expected map");
        }
    }
    #[test]
    fn list_literal() {
        if let GremlinValue::List(l) = parse_single_value("[1, 2, 3]", &|_| None).unwrap() {
            assert_eq!(l.len(), 3);
        } else {
            panic!("expected list");
        }
    }
    #[test]
    fn groovy_map_literal() {
        if let GremlinValue::Map(m) = parse_single_value("[\"k\": \"v\"]", &|_| None).unwrap() {
            assert_eq!(m.get("k"), Some(&GremlinValue::String("v".to_string())));
        } else {
            panic!("expected map");
        }
    }
    #[test]
    fn empty_groovy_map() {
        if let GremlinValue::Map(m) = parse_single_value("[:]", &|_| None).unwrap() {
            assert!(m.is_empty());
        } else {
            panic!("expected empty map");
        }
    }
    #[test]
    fn typed_d_int() {
        assert_eq!(
            parse_single_value("d[29].i", &|_| None).unwrap(),
            GremlinValue::Integer(29)
        );
    }
    #[test]
    fn typed_d_float() {
        assert_eq!(
            parse_single_value("d[0.5].d", &|_| None).unwrap(),
            GremlinValue::Float(0.5)
        );
    }
    #[test]
    fn t_label_token() {
        assert_eq!(
            parse_single_value("T.label", &|_| None).unwrap(),
            GremlinValue::String("~label".to_string())
        );
    }
    #[test]
    fn t_id_token() {
        assert_eq!(
            parse_single_value("T.id", &|_| None).unwrap(),
            GremlinValue::String("~id".to_string())
        );
    }
    #[test]
    fn t_label_bracket() {
        assert_eq!(
            parse_single_value("t[label]", &|_| None).unwrap(),
            GremlinValue::String("~label".to_string())
        );
    }
    #[test]
    fn t_id_bracket() {
        assert_eq!(
            parse_single_value("t[id]", &|_| None).unwrap(),
            GremlinValue::String("~id".to_string())
        );
    }
    #[test]
    fn scope_local() {
        assert_eq!(
            parse_single_value("Scope.local", &|_| None).unwrap(),
            GremlinValue::String("Scope.local".to_string())
        );
    }
    #[test]
    fn column_keys() {
        assert_eq!(
            parse_single_value("Column.keys", &|_| None).unwrap(),
            GremlinValue::String("Column.keys".to_string())
        );
    }
    #[test]
    fn order_desc() {
        assert_eq!(
            parse_single_value("Order.desc", &|_| None).unwrap(),
            GremlinValue::String("Order.desc".to_string())
        );
    }
    #[test]
    fn merge_token() {
        assert_eq!(
            parse_single_value("Merge.onMatch", &|_| None).unwrap(),
            GremlinValue::String("Merge.onMatch".to_string())
        );
    }
    #[test]
    fn pick_none() {
        assert_eq!(
            parse_single_value("Pick.none", &|_| None).unwrap(),
            GremlinValue::String("none".to_string())
        );
    }
    #[test]
    fn g_type_literal() {
        assert_eq!(
            parse_single_value("GType.i32", &|_| None).unwrap(),
            GremlinValue::String("GType.i32".to_string())
        );
    }
    #[test]
    fn datetime_literal() {
        assert_eq!(
            parse_single_value("datetime(\"2023-01-01T00:00:00Z\")", &|_| None).unwrap(),
            GremlinValue::String("__gremlin_dt__:2023-01-01T00:00:00Z".to_string())
        );
    }
    #[test]
    fn uuid_literal() {
        assert_eq!(
            parse_single_value("UUID(\"abc\")", &|_| None).unwrap(),
            GremlinValue::String("__gremlin_uuid__:abc".to_string())
        );
    }
    #[test]
    fn anonymous_traversal_as_string() {
        assert_eq!(
            parse_single_value("__.select(\"a\")", &|_| None).unwrap(),
            GremlinValue::String("__.select(\"a\")".to_string())
        );
    }
    #[test]
    fn unknown_falls_back_to_string() {
        assert_eq!(
            parse_single_value("foo_unknown_xyz", &|_| None).unwrap(),
            GremlinValue::String("foo_unknown_xyz".to_string())
        );
    }
    #[test]
    fn empty_input_is_null() {
        assert_eq!(
            parse_single_value("", &|_| None).unwrap(),
            GremlinValue::Null
        );
    }
    #[test]
    fn negative_integer() {
        assert_eq!(
            parse_single_value("-42", &|_| None).unwrap(),
            GremlinValue::Integer(-42)
        );
    }
    #[test]
    fn parameter_resolves() {
        let v = parse_single_value("param_x", &|n| {
            if n == "param_x" {
                Some(GremlinValue::Integer(99))
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(v, GremlinValue::Integer(99));
    }

    // parse_literal_value: unsuffixed paths
    #[test]
    fn lit_value_int_no_suffix() {
        assert_eq!(
            super::parse_literal_value("42", &|_| None),
            Some(GremlinValue::Integer(42))
        );
    }
    #[test]
    fn lit_value_float_no_suffix() {
        assert_eq!(
            super::parse_literal_value("0.5", &|_| None),
            Some(GremlinValue::Float(0.5))
        );
    }
    #[test]
    fn lit_value_quoted_double() {
        assert_eq!(
            super::parse_literal_value("\"x\"", &|_| None),
            Some(GremlinValue::String("x".to_string()))
        );
    }
    #[test]
    fn lit_value_quoted_single() {
        assert_eq!(
            super::parse_literal_value("'x'", &|_| None),
            Some(GremlinValue::String("x".to_string()))
        );
    }
    #[test]
    fn lit_value_true() {
        assert_eq!(
            super::parse_literal_value("true", &|_| None),
            Some(GremlinValue::Bool(true))
        );
    }
    #[test]
    fn lit_value_false() {
        assert_eq!(
            super::parse_literal_value("false", &|_| None),
            Some(GremlinValue::Bool(false))
        );
    }
    #[test]
    fn lit_value_null() {
        assert_eq!(
            super::parse_literal_value("null", &|_| None),
            Some(GremlinValue::Null)
        );
    }
    #[test]
    fn lit_value_param_resolves() {
        let v = super::parse_literal_value("p", &|n| {
            if n == "p" {
                Some(GremlinValue::Integer(7))
            } else {
                None
            }
        });
        assert_eq!(v, Some(GremlinValue::Integer(7)));
    }
    #[test]
    fn lit_value_long_suffix() {
        assert_eq!(
            super::parse_literal_value("1L", &|_| None),
            Some(GremlinValue::Integer(1))
        );
    }
    #[test]
    fn lit_value_double_suffix() {
        assert_eq!(
            super::parse_literal_value("0.5d", &|_| None),
            Some(GremlinValue::Float(0.5))
        );
    }
}
