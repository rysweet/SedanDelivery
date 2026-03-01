/// Bolt protocol value type.
///
/// Represents every data type that can appear on the Bolt wire.
/// Structures are kept in their tagged-fields form rather than
/// being pre-parsed into domain types so that the packstream layer
/// remains format-agnostic.
#[derive(Debug, Clone, PartialEq)]
pub enum BoltValue {
    /// The absence of a value.
    Null,
    /// A boolean value.
    Bool(bool),
    /// A 64-bit signed integer.
    Int(i64),
    /// A 64-bit IEEE 754 floating-point number.
    Float(f64),
    /// A raw byte array.
    Bytes(Vec<u8>),
    /// A UTF-8 string.
    String(String),
    /// An ordered sequence of values.
    List(Vec<BoltValue>),
    /// An ordered sequence of key-value pairs (keys are always strings).
    Map(Vec<(String, BoltValue)>),
    /// A tagged structure with positional fields (e.g. Node, Relationship,
    /// temporal types).
    Structure { tag: u8, fields: Vec<BoltValue> },
}

/// Type alias for a BoltValue::Map's entries.
pub type BoltMap = Vec<(String, BoltValue)>;

/// Helper methods for extracting values from a `BoltMap`.
pub trait BoltMapExt {
    /// Look up a value by key.
    fn get_value(&self, key: &str) -> Option<&BoltValue>;
    /// Look up a string value by key.
    fn get_str(&self, key: &str) -> Option<&str>;
    /// Look up an integer value by key.
    fn get_int(&self, key: &str) -> Option<i64>;
}

impl BoltMapExt for BoltMap {
    fn get_value(&self, key: &str) -> Option<&BoltValue> {
        self.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn get_str(&self, key: &str) -> Option<&str> {
        match self.get_value(key) {
            Some(BoltValue::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    fn get_int(&self, key: &str) -> Option<i64> {
        match self.get_value(key) {
            Some(BoltValue::Int(n)) => Some(*n),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bolt_value_null_equality() {
        assert_eq!(BoltValue::Null, BoltValue::Null);
    }

    #[test]
    fn bolt_value_clone() {
        let v = BoltValue::List(vec![BoltValue::Int(1), BoltValue::String("hello".into())]);
        let v2 = v.clone();
        assert_eq!(v, v2);
    }

    #[test]
    fn bolt_value_debug() {
        let v = BoltValue::Bool(true);
        let dbg = format!("{:?}", v);
        assert!(dbg.contains("Bool"));
    }

    #[test]
    fn bolt_map_get_value_found() {
        let map: BoltMap = vec![
            ("name".into(), BoltValue::String("Alice".into())),
            ("age".into(), BoltValue::Int(30)),
        ];
        assert_eq!(
            map.get_value("name"),
            Some(&BoltValue::String("Alice".into()))
        );
    }

    #[test]
    fn bolt_map_get_value_not_found() {
        let map: BoltMap = vec![];
        assert_eq!(map.get_value("missing"), None);
    }

    #[test]
    fn bolt_map_get_str() {
        let map: BoltMap = vec![
            ("name".into(), BoltValue::String("Alice".into())),
            ("age".into(), BoltValue::Int(30)),
        ];
        assert_eq!(map.get_str("name"), Some("Alice"));
        assert_eq!(map.get_str("age"), None);
        assert_eq!(map.get_str("missing"), None);
    }

    #[test]
    fn bolt_map_get_int() {
        let map: BoltMap = vec![
            ("name".into(), BoltValue::String("Alice".into())),
            ("age".into(), BoltValue::Int(30)),
        ];
        assert_eq!(map.get_int("age"), Some(30));
        assert_eq!(map.get_int("name"), None);
        assert_eq!(map.get_int("missing"), None);
    }

    #[test]
    fn bolt_map_duplicate_keys_returns_first() {
        let map: BoltMap = vec![
            ("k".into(), BoltValue::Int(1)),
            ("k".into(), BoltValue::Int(2)),
        ];
        assert_eq!(map.get_int("k"), Some(1));
    }

    #[test]
    fn structure_value() {
        let s = BoltValue::Structure {
            tag: 0x01,
            fields: vec![BoltValue::Int(42), BoltValue::String("test".into())],
        };
        if let BoltValue::Structure { tag, fields } = &s {
            assert_eq!(*tag, 0x01);
            assert_eq!(fields.len(), 2);
        } else {
            panic!("expected Structure");
        }
    }

    #[test]
    fn nested_map_in_list() {
        let inner: BoltMap = vec![("x".into(), BoltValue::Int(1))];
        let v = BoltValue::List(vec![BoltValue::Map(inner)]);
        if let BoltValue::List(items) = &v {
            if let BoltValue::Map(map) = &items[0] {
                assert_eq!(map.get_int("x"), Some(1));
            } else {
                panic!("expected Map");
            }
        } else {
            panic!("expected List");
        }
    }
}
