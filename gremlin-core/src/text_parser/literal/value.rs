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
    if s.len() > 1 {
        let suffix = s.as_bytes()[s.len() - 1];
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
