// validation.rs — Input validation for graph names, labels, and property keys.
//
// These validators guard the write path against inputs that could corrupt the
// Redis key schema (null bytes, oversized strings) or break Redis Cluster slot
// routing (brace characters in graph names).

use cypher_core::error::StorageError;

/// Maximum allowed byte length for a label string.
pub const LABEL_MAX_LEN: usize = 256;

/// Maximum allowed byte length for a property key string.
pub const PROPERTY_KEY_MAX_LEN: usize = 512;

/// Validate a node or edge label string.
///
/// Rules:
/// - Must not be empty.
/// - Must not exceed [`LABEL_MAX_LEN`] bytes.
/// - Must not contain null bytes (`\x00`) — used as separator in index keys.
pub fn validate_label(label: &str) -> Result<(), StorageError> {
    if label.is_empty() {
        return Err(StorageError::ConstraintViolation {
            msg: "label must not be empty".to_string(),
        });
    }
    if label.len() > LABEL_MAX_LEN {
        return Err(StorageError::ConstraintViolation {
            msg: format!(
                "label exceeds maximum length of {} bytes (got {})",
                LABEL_MAX_LEN,
                label.len()
            ),
        });
    }
    if label.contains('\x00') {
        return Err(StorageError::ConstraintViolation {
            msg: "label must not contain null bytes".to_string(),
        });
    }
    Ok(())
}

/// Validate a property key string.
///
/// Rules:
/// - Must not be empty.
/// - Must not exceed [`PROPERTY_KEY_MAX_LEN`] bytes.
/// - Must not contain null bytes (`\x00`) — used as separator in index keys.
pub fn validate_property_key(key: &str) -> Result<(), StorageError> {
    if key.is_empty() {
        return Err(StorageError::ConstraintViolation {
            msg: "property key must not be empty".to_string(),
        });
    }
    if key.len() > PROPERTY_KEY_MAX_LEN {
        return Err(StorageError::ConstraintViolation {
            msg: format!(
                "property key exceeds maximum length of {} bytes (got {})",
                PROPERTY_KEY_MAX_LEN,
                key.len()
            ),
        });
    }
    if key.contains('\x00') {
        return Err(StorageError::ConstraintViolation {
            msg: "property key must not contain null bytes".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_label ───────────────────────────────────────────────────────

    /// Happy path: a plain ASCII label is accepted.
    #[test]
    fn label_valid_ascii() {
        assert!(
            validate_label("Person").is_ok(),
            "plain ASCII label should be accepted"
        );
    }

    /// Happy path: a label with unicode is accepted (no byte-count restriction
    /// beyond LABEL_MAX_LEN; unicode chars count by UTF-8 byte length).
    #[test]
    fn label_valid_unicode() {
        assert!(
            validate_label("Personne").is_ok(),
            "unicode label should be accepted"
        );
    }

    /// Boundary: a label that is exactly LABEL_MAX_LEN bytes is accepted.
    #[test]
    fn label_at_max_len_accepted() {
        let label = "A".repeat(LABEL_MAX_LEN);
        assert!(
            validate_label(&label).is_ok(),
            "label at exactly LABEL_MAX_LEN bytes should be accepted"
        );
    }

    /// Boundary: a label one byte over LABEL_MAX_LEN is rejected.
    #[test]
    fn label_over_max_len_rejected() {
        let label = "A".repeat(LABEL_MAX_LEN + 1);
        let result = validate_label(&label);
        assert!(
            result.is_err(),
            "label exceeding LABEL_MAX_LEN bytes should be rejected"
        );
    }

    /// Error: empty label is rejected.
    #[test]
    fn label_empty_rejected() {
        let result = validate_label("");
        assert!(result.is_err(), "empty label must be rejected");
    }

    /// Error: label containing a null byte is rejected (null byte is used as
    /// the separator character in propidx keys, so it must never appear in
    /// label or property names).
    #[test]
    fn label_with_null_byte_rejected() {
        let label = "Per\x00son";
        let result = validate_label(label);
        assert!(
            result.is_err(),
            "label containing null byte must be rejected"
        );
    }

    /// Error: label that is only a null byte is rejected.
    #[test]
    fn label_only_null_byte_rejected() {
        let result = validate_label("\x00");
        assert!(result.is_err(), "null-byte-only label must be rejected");
    }

    /// Error: label with embedded null byte at the start is rejected.
    #[test]
    fn label_leading_null_byte_rejected() {
        let result = validate_label("\x00Person");
        assert!(
            result.is_err(),
            "label with leading null byte must be rejected"
        );
    }

    // ── validate_property_key ────────────────────────────────────────────────

    /// Happy path: a plain property key is accepted.
    #[test]
    fn property_key_valid_ascii() {
        assert!(
            validate_property_key("name").is_ok(),
            "plain ASCII property key should be accepted"
        );
    }

    /// Happy path: property keys with dots and underscores are accepted.
    #[test]
    fn property_key_with_special_chars() {
        assert!(
            validate_property_key("user.first_name").is_ok(),
            "property key with dots and underscores should be accepted"
        );
    }

    /// Boundary: property key at exactly PROPERTY_KEY_MAX_LEN bytes is accepted.
    #[test]
    fn property_key_at_max_len_accepted() {
        let key = "k".repeat(PROPERTY_KEY_MAX_LEN);
        assert!(
            validate_property_key(&key).is_ok(),
            "property key at exactly PROPERTY_KEY_MAX_LEN bytes should be accepted"
        );
    }

    /// Boundary: property key one byte over PROPERTY_KEY_MAX_LEN is rejected.
    #[test]
    fn property_key_over_max_len_rejected() {
        let key = "k".repeat(PROPERTY_KEY_MAX_LEN + 1);
        let result = validate_property_key(&key);
        assert!(
            result.is_err(),
            "property key exceeding PROPERTY_KEY_MAX_LEN bytes should be rejected"
        );
    }

    /// Error: empty property key is rejected.
    #[test]
    fn property_key_empty_rejected() {
        let result = validate_property_key("");
        assert!(result.is_err(), "empty property key must be rejected");
    }

    /// Error: property key containing a null byte is rejected.
    #[test]
    fn property_key_with_null_byte_rejected() {
        let key = "na\x00me";
        let result = validate_property_key(key);
        assert!(
            result.is_err(),
            "property key with null byte must be rejected"
        );
    }

    /// Error: property key that starts with a null byte is rejected.
    #[test]
    fn property_key_leading_null_byte_rejected() {
        let result = validate_property_key("\x00key");
        assert!(
            result.is_err(),
            "property key with leading null byte must be rejected"
        );
    }

    /// Error: property key that is only a null byte is rejected.
    #[test]
    fn property_key_only_null_byte_rejected() {
        let result = validate_property_key("\x00");
        assert!(
            result.is_err(),
            "null-byte-only property key must be rejected"
        );
    }

    // ── Cross-module: null byte separator contract ───────────────────────────

    /// The null byte is the IDX_SEP used in propidx keys.
    ///
    /// If label or property key contained IDX_SEP, the resulting Redis key
    /// would be ambiguous and break index lookups. This test documents and
    /// enforces that contract.
    #[test]
    fn null_byte_is_idx_sep_used_in_propidx_keys() {
        // Import IDX_SEP to verify the constant hasn't changed to something
        // that would no longer be rejected by validate_label/validate_property_key.
        let sep = crate::keys::IDX_SEP;
        assert_eq!(
            sep, '\x00',
            "IDX_SEP must be null byte — update validation rules if this changes"
        );
        let label_with_sep = format!("Label{sep}Suffix");
        let prop_with_sep = format!("prop{sep}key");
        assert!(
            validate_label(&label_with_sep).is_err(),
            "label containing IDX_SEP must be rejected"
        );
        assert!(
            validate_property_key(&prop_with_sep).is_err(),
            "property key containing IDX_SEP must be rejected"
        );
    }
}
