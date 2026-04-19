//! Graph-name validation.
//!
//! Extracted from `storage/mod.rs` so the trait module focuses on the
//! [`GraphStore`](super::GraphStore) contract. Re-exported from
//! [`crate::storage`] to preserve the public API.

use crate::error::StorageError;

/// Default maximum allowed length for a graph name.
///
/// The effective limit at runtime is [`crate::config::global().graph_name_max_len`]
/// which is injectable via the `GQL_GRAPH_NAME_MAX_LEN` environment variable.
pub const GRAPH_NAME_MAX_LEN: usize = 64;

/// Validate a graph name. Rules (security rationale in docs/RESEARCH.md):
/// - Non-empty
/// - At most `GQL_GRAPH_NAME_MAX_LEN` bytes (default 64)
/// - No `:` (Redis key separator — would break the `G:node:{id}` key namespace)
/// - No `{` or `}` (Redis Cluster hash tags — would corrupt slot routing)
/// - Characters: `[a-zA-Z0-9_.-]` only (whitelist)
pub fn validate_graph_name(name: &str) -> Result<(), StorageError> {
    if name.is_empty() {
        return Err(StorageError::InvalidGraphName {
            name: name.to_string(),
            reason: "graph name must not be empty",
        });
    }
    let max_len = crate::config::global().graph_name_max_len;
    if name.len() > max_len {
        return Err(StorageError::InvalidGraphName {
            name: name.to_string(),
            reason: "graph name exceeds the configured maximum length",
        });
    }
    if name.contains(':') {
        return Err(StorageError::InvalidGraphName {
            name: name.to_string(),
            reason: "graph name must not contain ':' (reserved as Redis key separator)",
        });
    }
    if name.contains('{') || name.contains('}') {
        return Err(StorageError::InvalidGraphName {
            name: name.to_string(),
            reason: "graph name must not contain '{' or '}' (reserved as Redis hash tags)",
        });
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(StorageError::InvalidGraphName {
            name: name.to_string(),
            reason: "graph name may only contain [a-zA-Z0-9_.-]",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_graph_names() {
        assert!(validate_graph_name("social").is_ok());
        assert!(validate_graph_name("my-graph").is_ok());
        assert!(validate_graph_name("graph_1").is_ok());
    }

    #[test]
    fn invalid_graph_names() {
        // Empty
        assert!(validate_graph_name("").is_err());
        // Contains colon
        assert!(validate_graph_name("foo:bar").is_err());
        // Contains Redis hash tag
        assert!(validate_graph_name("{myslot}graph").is_err());
        assert!(validate_graph_name("graph}end").is_err());
    }
}
