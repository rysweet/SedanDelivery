//! URL path-segment parsers for node and edge identifiers.

use cypher_core::storage::{EdgeId, NodeId};

use crate::error::ApiError;

/// Parse a URL path segment as a `NodeId`.
pub fn parse_node_id(s: &str) -> Result<NodeId, ApiError> {
    s.parse::<u64>()
        .map(NodeId)
        .map_err(|_| ApiError::bad_request(format!("invalid node id: '{s}'")))
}

/// Parse a URL path segment as an `EdgeId`.
pub fn parse_edge_id(s: &str) -> Result<EdgeId, ApiError> {
    s.parse::<u64>()
        .map(EdgeId)
        .map_err(|_| ApiError::bad_request(format!("invalid edge id: '{s}'")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_node_id ────────────────────────────────────────────────────────

    #[test]
    fn parse_node_id_valid() {
        let result = parse_node_id("42").unwrap();
        assert_eq!(result, NodeId(42));
    }

    #[test]
    fn parse_node_id_zero() {
        let result = parse_node_id("0").unwrap();
        assert_eq!(result, NodeId(0));
    }

    #[test]
    fn parse_node_id_large() {
        let result = parse_node_id("18446744073709551615").unwrap();
        assert_eq!(result, NodeId(u64::MAX));
    }

    #[test]
    fn parse_node_id_invalid_string() {
        let result = parse_node_id("abc");
        assert!(result.is_err());
    }

    #[test]
    fn parse_node_id_negative() {
        let result = parse_node_id("-1");
        assert!(result.is_err());
    }

    #[test]
    fn parse_node_id_empty() {
        let result = parse_node_id("");
        assert!(result.is_err());
    }

    #[test]
    fn parse_node_id_float_string() {
        let result = parse_node_id("1.5");
        assert!(result.is_err());
    }

    // ── parse_edge_id ────────────────────────────────────────────────────────

    #[test]
    fn parse_edge_id_valid() {
        let result = parse_edge_id("99").unwrap();
        assert_eq!(result, EdgeId(99));
    }

    #[test]
    fn parse_edge_id_zero() {
        let result = parse_edge_id("0").unwrap();
        assert_eq!(result, EdgeId(0));
    }

    #[test]
    fn parse_edge_id_invalid() {
        let result = parse_edge_id("not-a-number");
        assert!(result.is_err());
    }

    #[test]
    fn parse_edge_id_negative() {
        let result = parse_edge_id("-42");
        assert!(result.is_err());
    }

    #[test]
    fn parse_edge_id_overflow() {
        let result = parse_edge_id("18446744073709551616");
        assert!(result.is_err());
    }
}
