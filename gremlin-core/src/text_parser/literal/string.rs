//! String unquoting and string-token normalization helpers.

/// Remove surrounding double quotes from a string, if present.
pub(crate) fn unquote(s: &str) -> String {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
        s[1..s.len() - 1].replace("\\'", "'").replace("\\\\", "\\")
    } else {
        s.to_string()
    }
}

/// Resolve `__.constant("X")` or `constant("X")` traversal patterns to the
/// literal string "X". Used by select() to support traversal-based keys.
pub(crate) fn resolve_constant_traversal(s: &str) -> String {
    let inner = s
        .strip_prefix("__.constant(")
        .or_else(|| s.strip_prefix("constant("));
    if let Some(rest) = inner {
        if let Some(body) = rest.strip_suffix(')') {
            return unquote(body.trim());
        }
    }
    s.to_string()
}

/// Normalize a GType name to the Gremlin type string used by typeOf().
pub(crate) fn normalize_gtype(s: &str) -> String {
    match s.to_uppercase().as_str() {
        "STRING" | "JAVA_LANG_STRING" => "string".to_string(),
        "INT" | "INTEGER" | "JAVA_LANG_INTEGER" => "integer".to_string(),
        "LONG" | "JAVA_LANG_LONG" => "long".to_string(),
        "DOUBLE" | "JAVA_LANG_DOUBLE" => "double".to_string(),
        "FLOAT" | "JAVA_LANG_FLOAT" => "float".to_string(),
        "BOOLEAN" | "JAVA_LANG_BOOLEAN" => "boolean".to_string(),
        "MAP" | "JAVA_UTIL_MAP" => "map".to_string(),
        "LIST" | "JAVA_UTIL_LIST" => "list".to_string(),
        "SET" | "JAVA_UTIL_SET" => "set".to_string(),
        "SHORT" | "JAVA_LANG_SHORT" => "short".to_string(),
        "BYTE" | "JAVA_LANG_BYTE" => "byte".to_string(),
        "BIGINTEGER" | "JAVA_MATH_BIGINTEGER" => "biginteger".to_string(),
        "BIGDECIMAL" | "JAVA_MATH_BIGDECIMAL" => "bigdecimal".to_string(),
        other => other.to_lowercase(),
    }
}
