// writes/helpers.rs -- Pure helper functions shared across write sub-modules.

use indexmap::IndexMap;

use gql_core::error::StorageError;
use gql_core::types::Value;

/// Extract a FanoutStore-injected ID from properties, if present.
///
/// Looks for `key` in `properties`. If the value is `Value::Int(i)` with
/// `i >= 0`, returns `Some(i as u64)`. If `i < 0`, returns an error. If
/// the key is absent or the value is not an integer, returns `None`.
///
/// This is used for both `_sd2_id` (node IDs) and `_sd2_eid` (edge IDs).
pub(crate) fn extract_injected_id(
    properties: &IndexMap<String, Value>,
    key: &str,
) -> Result<Option<u64>, StorageError> {
    if let Some(Value::Int(i)) = properties.get(key) {
        if *i < 0 {
            crate::record_backend_error();
            return Err(StorageError::Backend {
                msg: format!("{key} must be non-negative, got {i}"),
            });
        }
        Ok(Some(*i as u64))
    } else {
        Ok(None)
    }
}

/// Strip internal FanoutStore keys (`_sd2_id`, `_sd2_eid`) from a properties map.
///
/// These keys are used for cross-cluster ID coordination and must not be
/// persisted as user-visible properties.
pub(crate) fn clean_fanout_props(
    mut properties: IndexMap<String, Value>,
) -> IndexMap<String, Value> {
    properties.shift_remove("_sd2_id");
    properties.shift_remove("_sd2_eid");
    properties
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- extract_injected_id ------------------------------------------------

    #[test]
    fn extract_positive_int() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(100));
        assert_eq!(extract_injected_id(&props, "_sd2_id").unwrap(), Some(100));
    }

    #[test]
    fn extract_zero() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(0));
        assert_eq!(extract_injected_id(&props, "_sd2_id").unwrap(), Some(0));
    }

    #[test]
    fn extract_negative_returns_error() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(-10));
        let err = extract_injected_id(&props, "_sd2_id").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("-10"), "error should mention the bad value");
    }

    #[test]
    fn extract_absent_key_returns_none() {
        let props = IndexMap::new();
        assert_eq!(extract_injected_id(&props, "_sd2_id").unwrap(), None);
    }

    #[test]
    fn extract_non_int_type_returns_none() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Str("hello".into()));
        assert_eq!(extract_injected_id(&props, "_sd2_id").unwrap(), None);
    }

    #[test]
    fn extract_bool_type_returns_none() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Bool(true));
        assert_eq!(extract_injected_id(&props, "_sd2_id").unwrap(), None);
    }

    #[test]
    fn extract_max_u64_via_i64() {
        let mut props = IndexMap::new();
        // i64::MAX as the largest representable positive value.
        props.insert("_sd2_eid".to_string(), Value::Int(i64::MAX));
        assert_eq!(
            extract_injected_id(&props, "_sd2_eid").unwrap(),
            Some(i64::MAX as u64)
        );
    }

    // ---- clean_fanout_props -------------------------------------------------

    #[test]
    fn clean_removes_both_internal_keys() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(1));
        props.insert("_sd2_eid".to_string(), Value::Int(2));
        props.insert("name".to_string(), Value::Str("Bob".into()));
        props.insert("age".to_string(), Value::Int(30));
        let cleaned = clean_fanout_props(props);
        assert_eq!(cleaned.len(), 2);
        assert!(cleaned.contains_key("name"));
        assert!(cleaned.contains_key("age"));
    }

    #[test]
    fn clean_noop_when_no_internal_keys() {
        let mut props = IndexMap::new();
        props.insert("x".to_string(), Value::Int(1));
        let original_len = props.len();
        let cleaned = clean_fanout_props(props);
        assert_eq!(cleaned.len(), original_len);
    }

    #[test]
    fn clean_empty_map() {
        let cleaned = clean_fanout_props(IndexMap::new());
        assert!(cleaned.is_empty());
    }

    #[test]
    fn clean_only_internal_keys() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(1));
        props.insert("_sd2_eid".to_string(), Value::Int(2));
        let cleaned = clean_fanout_props(props);
        assert!(cleaned.is_empty());
    }
}
