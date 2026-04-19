//! Inherent accessor and coercion methods on [`Value`].
//!
//! These are split out of `mod.rs` to keep the central enum definition small.

use super::Value;

impl Value {
    /// Returns a static string naming the Cypher type of this value.
    #[inline]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "Null",
            Self::Bool(_) => "Boolean",
            Self::Int(_) => "Integer",
            Self::Float(_) => "Float",
            Self::Str(_) => "String",
            Self::Date(_) => "Date",
            Self::LocalTime(_) => "LocalTime",
            Self::Time(_, _) => "Time",
            Self::LocalDateTime(_) => "LocalDateTime",
            Self::DateTime(_) => "DateTime",
            Self::Duration(_) => "Duration",
            Self::Point(_) => "Point",
            Self::Bytes(_) => "ByteArray",
            Self::List(_) => "List",
            Self::Map(_) => "Map",
            Self::Node(_) => "Node",
            Self::Edge(_) => "Relationship",
            Self::Path { .. } => "Path",
        }
    }

    /// Returns `true` if this value is `Null`.
    #[inline]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Coerce to `f64` for arithmetic. Returns `None` for non-numeric types.
    #[inline]
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(f) => Some(*f),
            Self::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// Coerce to `i64`. Returns `None` for non-integer types (does NOT truncate floats).
    #[inline]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Returns the string slice if this is a `Str` variant.
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Returns the boolean value if this is a `Bool` variant.
    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Cypher truthiness: only `false` and `Null` are falsy.
    /// Everything else (including 0, empty string, empty list) is truthy.
    #[inline]
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_is_falsy() {
        assert!(!Value::Null.is_truthy());
    }

    #[test]
    fn false_is_falsy() {
        assert!(!Value::Bool(false).is_truthy());
    }

    #[test]
    fn zero_int_is_truthy() {
        // Cypher: 0 is truthy (unlike Python/JS)
        assert!(Value::Int(0).is_truthy());
    }

    #[test]
    fn empty_string_is_truthy() {
        // Cypher: "" is truthy (unlike Python/JS)
        assert!(Value::Str("".into()).is_truthy());
    }

    #[test]
    fn type_names_correct() {
        assert_eq!(Value::Null.type_name(), "Null");
        assert_eq!(Value::Bool(true).type_name(), "Boolean");
        assert_eq!(Value::Int(1).type_name(), "Integer");
        assert_eq!(Value::Float(1.0).type_name(), "Float");
        assert_eq!(Value::Str("x".into()).type_name(), "String");
    }

    #[test]
    fn as_float_coerces_int() {
        assert_eq!(Value::Int(3).as_float(), Some(3.0));
        assert_eq!(Value::Float(1.23456).as_float(), Some(1.23456));
        assert_eq!(Value::Null.as_float(), None);
    }
}
