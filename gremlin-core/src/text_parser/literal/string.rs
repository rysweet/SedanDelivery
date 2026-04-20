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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquote_double_quoted() {
        assert_eq!(unquote("\"hello\""), "hello");
    }
    #[test]
    fn unquote_double_quoted_with_escape() {
        assert_eq!(unquote("\"a\\\"b\""), "a\"b");
    }
    #[test]
    fn unquote_double_quoted_with_backslash() {
        assert_eq!(unquote("\"a\\\\b\""), "a\\b");
    }
    #[test]
    fn unquote_single_quoted() {
        assert_eq!(unquote("'hello'"), "hello");
    }
    #[test]
    fn unquote_single_quoted_with_escape() {
        assert_eq!(unquote("'a\\'b'"), "a'b");
    }
    #[test]
    fn unquote_single_quoted_with_backslash() {
        assert_eq!(unquote("'a\\\\b'"), "a\\b");
    }
    #[test]
    fn unquote_unquoted_passthrough() {
        assert_eq!(unquote("nope"), "nope");
    }
    #[test]
    fn unquote_empty_passthrough() {
        assert_eq!(unquote(""), "");
    }
    #[test]
    fn unquote_only_one_quote_passthrough() {
        assert_eq!(unquote("\""), "\"");
    }

    #[test]
    fn resolve_constant_dunder_form() {
        assert_eq!(resolve_constant_traversal("__.constant(\"X\")"), "X");
    }
    #[test]
    fn resolve_constant_bare_form() {
        assert_eq!(resolve_constant_traversal("constant(\"Y\")"), "Y");
    }
    #[test]
    fn resolve_constant_single_quoted() {
        assert_eq!(resolve_constant_traversal("constant('Z')"), "Z");
    }
    #[test]
    fn resolve_constant_no_match_passthrough() {
        assert_eq!(resolve_constant_traversal("foo(\"bar\")"), "foo(\"bar\")");
    }
    #[test]
    fn resolve_constant_no_close_paren_passthrough() {
        // Missing trailing ')' — falls back to original string.
        assert_eq!(
            resolve_constant_traversal("constant(\"X\""),
            "constant(\"X\""
        );
    }

    #[test]
    fn normalize_gtype_string_variants() {
        assert_eq!(normalize_gtype("STRING"), "string");
        assert_eq!(normalize_gtype("Java_Lang_String"), "string");
    }
    #[test]
    fn normalize_gtype_int_variants() {
        assert_eq!(normalize_gtype("INT"), "integer");
        assert_eq!(normalize_gtype("INTEGER"), "integer");
        assert_eq!(normalize_gtype("JAVA_LANG_INTEGER"), "integer");
    }
    #[test]
    fn normalize_gtype_long_double_float_bool() {
        assert_eq!(normalize_gtype("LONG"), "long");
        assert_eq!(normalize_gtype("DOUBLE"), "double");
        assert_eq!(normalize_gtype("FLOAT"), "float");
        assert_eq!(normalize_gtype("BOOLEAN"), "boolean");
    }
    #[test]
    fn normalize_gtype_collection() {
        assert_eq!(normalize_gtype("MAP"), "map");
        assert_eq!(normalize_gtype("LIST"), "list");
        assert_eq!(normalize_gtype("SET"), "set");
    }
    #[test]
    fn normalize_gtype_short_byte() {
        assert_eq!(normalize_gtype("SHORT"), "short");
        assert_eq!(normalize_gtype("BYTE"), "byte");
    }
    #[test]
    fn normalize_gtype_bigint_bigdec() {
        assert_eq!(normalize_gtype("BIGINTEGER"), "biginteger");
        assert_eq!(normalize_gtype("BIGDECIMAL"), "bigdecimal");
    }
    #[test]
    fn normalize_gtype_unknown_lowercased() {
        assert_eq!(normalize_gtype("UnknownType"), "unknowntype");
    }
}
