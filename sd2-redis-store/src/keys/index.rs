// Property-index keys and value-to-key encoding.

use cypher_core::types::Value;

/// Separator for label/property in index metadata and propidx keys.
/// Using a null byte avoids collisions when label or property names
/// themselves contain colons.
pub(crate) const IDX_SEP: char = '\x00';

/// Property index key: maps (label, property, value) -> SET of node IDs.
/// Enables O(1) MERGE lookups instead of O(N) full-label scans.
pub(crate) fn key_propidx(g: &str, label: &str, property: &str, value_key: &str) -> String {
    format!(
        "G:{{{g}}}:propidx:{label}{}{property}{}{value_key}",
        IDX_SEP, IDX_SEP
    )
}

pub(crate) fn key_propidx_prefix(g: &str) -> String {
    format!("G:{{{g}}}:propidx:")
}

/// Manifest of propidx keys created for a graph.
///
/// Index backfill and indexed writes are intentionally non-atomic, so
/// `delete_graph` needs a durable way to find propidx sets even if index
/// metadata registration fails part-way through.
pub(crate) fn key_propidx_manifest(g: &str) -> String {
    format!("G:{{{g}}}:_propidx")
}

/// Convert a Value to a canonical string for use as a Redis key component.
/// Must be deterministic: same value always produces same string.
pub(crate) fn value_to_index_key(val: &Value) -> String {
    match val {
        Value::Int(i) => format!("i:{i}"),
        Value::Float(f) => format!("f:{}", f.to_bits()), // bitwise for determinism
        Value::Str(s) => format!("s:{s}"),
        Value::Bool(b) => format!("b:{b}"),
        // For complex types, use Debug representation.
        other => format!("j:{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_propidx_format() {
        assert_eq!(
            key_propidx("db", "Person", "name", "s:Alice"),
            "G:{db}:propidx:Person\x00name\x00s:Alice"
        );
    }

    #[test]
    fn key_propidx_prefix_format() {
        assert_eq!(key_propidx_prefix("db"), "G:{db}:propidx:");
    }

    #[test]
    fn key_propidx_manifest_format() {
        assert_eq!(key_propidx_manifest("db"), "G:{db}:_propidx");
    }

    #[test]
    fn value_to_index_key_int() {
        assert_eq!(value_to_index_key(&Value::Int(42)), "i:42");
        assert_eq!(value_to_index_key(&Value::Int(-1)), "i:-1");
        assert_eq!(value_to_index_key(&Value::Int(0)), "i:0");
    }

    #[test]
    fn value_to_index_key_float() {
        let k = value_to_index_key(&Value::Float(std::f64::consts::PI));
        assert!(k.starts_with("f:"));
        // Determinism: same float produces same key.
        assert_eq!(k, value_to_index_key(&Value::Float(std::f64::consts::PI)));
    }

    #[test]
    fn value_to_index_key_string() {
        assert_eq!(value_to_index_key(&Value::Str("hello".into())), "s:hello");
        assert_eq!(value_to_index_key(&Value::Str("".into())), "s:");
    }

    #[test]
    fn value_to_index_key_bool() {
        assert_eq!(value_to_index_key(&Value::Bool(true)), "b:true");
        assert_eq!(value_to_index_key(&Value::Bool(false)), "b:false");
    }

    #[test]
    fn value_to_index_key_null_uses_debug() {
        let k = value_to_index_key(&Value::Null);
        assert!(k.starts_with("j:"));
    }

    #[test]
    fn value_to_index_key_deterministic_for_same_float() {
        // NaN bits: ensure bitwise representation is used.
        let a = value_to_index_key(&Value::Float(f64::NAN));
        let b = value_to_index_key(&Value::Float(f64::NAN));
        assert_eq!(a, b, "NaN must produce the same key every time");
    }
}
