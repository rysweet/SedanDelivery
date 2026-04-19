//! Core value types: `Point`, `SedanPath`, and `SedanValue`.

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
}
