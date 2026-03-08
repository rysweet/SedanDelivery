/// Tagged value decoder and graph-specific types.

/// A geographic or Cartesian point.
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
    pub crs: String,
}

/// A graph path consisting of node IDs and edge IDs.
#[derive(Debug, Clone, PartialEq)]
pub struct SedanPath {
    pub nodes: Vec<u64>,
    pub edges: Vec<u64>,
}

/// An enum representing values that can be used as Cypher query parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum SedanValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<SedanValue>),
}

impl SedanValue {
    /// Convert this value to a Cypher literal string.
    pub fn to_cypher(&self) -> String {
        match self {
            SedanValue::Null => "null".to_string(),
            SedanValue::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            SedanValue::Int(i) => i.to_string(),
            SedanValue::Float(f) => f.to_string(),
            SedanValue::Str(s) => {
                let mut escaped = String::with_capacity(s.len() + 2);
                for ch in s.chars() {
                    match ch {
                        '\\' => escaped.push_str("\\\\"),
                        '\'' => escaped.push_str("\\'"),
                        '\n' => escaped.push_str("\\n"),
                        '\r' => escaped.push_str("\\r"),
                        '\t' => escaped.push_str("\\t"),
                        '\0' => escaped.push_str("\\0"),
                        c if c.is_control() => {
                            // Remaining control characters (0x01-0x1F excl. already matched)
                            escaped.push_str(&format!("\\u{:04X}", c as u32));
                        }
                        c => escaped.push(c),
                    }
                }
                format!("'{}'", escaped)
            }
            SedanValue::List(items) => {
                let inner: Vec<String> = items.iter().map(|v| v.to_cypher()).collect();
                format!("[{}]", inner.join(", "))
            }
        }
    }
}

/// Substitute `$key` parameters in a Cypher query string with escaped values.
///
/// Only keys present in `params` are substituted; unknown `$` references are
/// left as-is so the server can report the error.
///
/// Uses a single-pass scan to prevent re-substitution (a replaced value
/// containing `$other` will NOT be replaced again). At each `$` position,
/// the longest matching parameter name is selected to avoid substring
/// collisions (e.g., `$name` vs `$nameLong`).
pub fn substitute_params(cypher: &str, params: &[(&str, SedanValue)]) -> String {
    if params.is_empty() {
        return cypher.to_string();
    }
    // Sort params longest-name-first so we try the longest match first at each $.
    let mut sorted: Vec<(&str, &SedanValue)> = params.iter().map(|(k, v)| (*k, v)).collect();
    sorted.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    let mut result = String::with_capacity(cypher.len());
    let bytes = cypher.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            // Try to match the longest parameter name at this position.
            let rest = &cypher[i + 1..];
            let mut matched = false;
            for (name, value) in &sorted {
                if rest.starts_with(name) {
                    // Word-boundary check: the char after the param name must not
                    // be alphanumeric or underscore.
                    let after_idx = i + 1 + name.len();
                    let at_boundary = cypher[after_idx..]
                        .chars()
                        .next()
                        .map_or(true, |c| !c.is_alphanumeric() && c != '_');
                    if at_boundary {
                        result.push_str(&value.to_cypher());
                        i = after_idx;
                        matched = true;
                        break;
                    }
                }
            }
            if !matched {
                result.push('$');
                i += 1;
            }
        } else {
            let ch = cypher[i..]
                .chars()
                .next()
                .expect("byte index is at valid UTF-8 boundary");
            result.push(ch);
            i += ch.len_utf8();
        }
    }
    result
}

/// Recursively decode tagged strings from the SedanDelivery wire format.
///
/// Tagged string prefixes:
/// - `__str:` -> plain string (un-escapes `__`-prefixed strings)
/// - `__date:` -> string value of the date
/// - `__localtime:` -> string value
/// - `__localdatetime:` -> string value
/// - `__datetime:` -> string value
/// - `__duration:` -> string value
/// - `__point:` -> JSON object with x, y, crs
/// - `__node:` -> integer node ID
/// - `__edge:` -> integer edge ID
///
/// Path objects (`__path_nodes` + `__path_edges`) are decoded into a JSON object
/// with "nodes" and "edges" arrays.
pub fn decode_value(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => decode_tagged_string(s),
        serde_json::Value::Array(arr) => {
            let decoded: Vec<serde_json::Value> = arr.iter().map(decode_value).collect();
            serde_json::Value::Array(decoded)
        }
        serde_json::Value::Object(map) => {
            // Check for path encoding first.
            if map.contains_key("__path_nodes") && map.contains_key("__path_edges") {
                let nodes = map
                    .get("__path_nodes")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|n| n.as_u64())
                            .map(serde_json::Value::from)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let edges = map
                    .get("__path_edges")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|e| e.as_u64())
                            .map(serde_json::Value::from)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let mut result = serde_json::Map::new();
                result.insert("nodes".to_string(), serde_json::Value::Array(nodes));
                result.insert("edges".to_string(), serde_json::Value::Array(edges));
                return serde_json::Value::Object(result);
            }
            let decoded: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, val)| (k.clone(), decode_value(val)))
                .collect();
            serde_json::Value::Object(decoded)
        }
        // Null, Bool, Number -- return as-is.
        other => other.clone(),
    }
}

fn decode_tagged_string(s: &str) -> serde_json::Value {
    if let Some(rest) = s.strip_prefix("__str:") {
        return serde_json::Value::String(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("__date:") {
        return serde_json::Value::String(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("__localtime:") {
        return serde_json::Value::String(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("__localdatetime:") {
        return serde_json::Value::String(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("__datetime:") {
        // Strip optional IANA tz suffix like [Europe/Berlin]
        let clean = if let Some(bracket) = rest.find('[') {
            &rest[..bracket]
        } else {
            rest
        };
        return serde_json::Value::String(clean.to_string());
    }
    if let Some(rest) = s.strip_prefix("__duration:") {
        return serde_json::Value::String(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("__point:") {
        let parts: Vec<&str> = rest.splitn(3, ',').collect();
        if parts.len() == 3 {
            if let (Ok(x), Ok(y)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>()) {
                let mut map = serde_json::Map::new();
                map.insert("x".to_string(), serde_json::Value::from(x));
                map.insert("y".to_string(), serde_json::Value::from(y));
                map.insert(
                    "crs".to_string(),
                    serde_json::Value::String(parts[2].to_string()),
                );
                return serde_json::Value::Object(map);
            }
        }
        // Malformed point, return as-is
        return serde_json::Value::String(s.to_string());
    }
    if let Some(rest) = s.strip_prefix("__node:") {
        if let Ok(id) = rest.parse::<u64>() {
            return serde_json::Value::from(id);
        }
    }
    if let Some(rest) = s.strip_prefix("__edge:") {
        if let Ok(id) = rest.parse::<u64>() {
            return serde_json::Value::from(id);
        }
    }
    // Not a tagged string -- return as-is.
    serde_json::Value::String(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sedan_value_null() {
        assert_eq!(SedanValue::Null.to_cypher(), "null");
    }

    #[test]
    fn test_sedan_value_bool() {
        assert_eq!(SedanValue::Bool(true).to_cypher(), "true");
        assert_eq!(SedanValue::Bool(false).to_cypher(), "false");
    }

    #[test]
    fn test_sedan_value_int() {
        assert_eq!(SedanValue::Int(42).to_cypher(), "42");
        assert_eq!(SedanValue::Int(-1).to_cypher(), "-1");
    }

    #[test]
    fn test_sedan_value_float() {
        assert_eq!(SedanValue::Float(3.14).to_cypher(), "3.14");
    }

    #[test]
    fn test_sedan_value_str() {
        assert_eq!(SedanValue::Str("Alice".into()).to_cypher(), "'Alice'");
    }

    #[test]
    fn test_sedan_value_str_escape_quotes() {
        assert_eq!(SedanValue::Str("O'Brien".into()).to_cypher(), "'O\\'Brien'");
    }

    #[test]
    fn test_sedan_value_str_escape_backslash() {
        assert_eq!(SedanValue::Str("a\\b".into()).to_cypher(), "'a\\\\b'");
    }

    #[test]
    fn test_sedan_value_str_escape_newline() {
        assert_eq!(
            SedanValue::Str("line1\nline2".into()).to_cypher(),
            "'line1\\nline2'"
        );
    }

    #[test]
    fn test_sedan_value_str_escape_carriage_return() {
        assert_eq!(SedanValue::Str("a\rb".into()).to_cypher(), "'a\\rb'");
    }

    #[test]
    fn test_sedan_value_str_escape_tab() {
        assert_eq!(
            SedanValue::Str("col1\tcol2".into()).to_cypher(),
            "'col1\\tcol2'"
        );
    }

    #[test]
    fn test_sedan_value_str_escape_null_byte() {
        assert_eq!(SedanValue::Str("a\0b".into()).to_cypher(), "'a\\0b'");
    }

    #[test]
    fn test_sedan_value_str_escape_control_char() {
        // BEL (0x07) should be escaped as \u0007
        let s = String::from("a\x07b");
        assert_eq!(SedanValue::Str(s).to_cypher(), "'a\\u0007b'");
    }

    #[test]
    fn test_sedan_value_str_escape_multiple_specials() {
        let s = String::from("O'Brien\tnewline\nback\\slash");
        assert_eq!(
            SedanValue::Str(s).to_cypher(),
            "'O\\'Brien\\tnewline\\nback\\\\slash'"
        );
    }

    #[test]
    fn test_sedan_value_list() {
        let v = SedanValue::List(vec![
            SedanValue::Int(1),
            SedanValue::Int(2),
            SedanValue::Int(3),
        ]);
        assert_eq!(v.to_cypher(), "[1, 2, 3]");
    }

    #[test]
    fn test_sedan_value_nested_list() {
        let v = SedanValue::List(vec![
            SedanValue::Int(1),
            SedanValue::List(vec![SedanValue::Int(2), SedanValue::Int(3)]),
        ]);
        assert_eq!(v.to_cypher(), "[1, [2, 3]]");
    }

    #[test]
    fn test_substitute_params_basic() {
        let result = substitute_params(
            "MATCH (n {name: $name})",
            &[("name", SedanValue::Str("Alice".into()))],
        );
        assert_eq!(result, "MATCH (n {name: 'Alice'})");
    }

    #[test]
    fn test_substitute_params_multiple() {
        let result = substitute_params(
            "WHERE n.age > $age AND n.active = $active",
            &[
                ("age", SedanValue::Int(21)),
                ("active", SedanValue::Bool(true)),
            ],
        );
        assert_eq!(result, "WHERE n.age > 21 AND n.active = true");
    }

    #[test]
    fn test_substitute_params_unknown_left_intact() {
        let result = substitute_params("$known $unknown", &[("known", SedanValue::Int(42))]);
        assert_eq!(result, "42 $unknown");
    }

    #[test]
    fn test_substitute_params_empty() {
        let result = substitute_params("MATCH (n) RETURN n", &[]);
        assert_eq!(result, "MATCH (n) RETURN n");
    }

    #[test]
    fn test_substitute_params_no_substring_collision() {
        // $name must NOT match inside $nameLong
        let result = substitute_params(
            "WHERE n.name = $name AND n.nameLong = $nameLong",
            &[
                ("name", SedanValue::Str("Alice".into())),
                ("nameLong", SedanValue::Str("Bob".into())),
            ],
        );
        assert_eq!(result, "WHERE n.name = 'Alice' AND n.nameLong = 'Bob'");
    }

    #[test]
    fn test_substitute_params_no_resubstitution() {
        // A replaced value containing $other must NOT be re-replaced
        let result = substitute_params(
            "RETURN $a, $b",
            &[
                ("a", SedanValue::Str("$b".into())),
                ("b", SedanValue::Int(99)),
            ],
        );
        assert_eq!(result, "RETURN '$b', 99");
    }
}
