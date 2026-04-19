// Adjacency set member encoding/decoding.

use cypher_core::storage::{AdjEntry, EdgeId, NodeId};

/// Encode an adjacency set member: "{edge_id}:{neighbor_id}:{rel_type}"
pub(crate) fn encode_adj(edge_id: EdgeId, neighbor_id: NodeId, rel_type: &str) -> String {
    format!("{}:{}:{}", edge_id.0, neighbor_id.0, rel_type)
}

/// Decode an adjacency set member. Returns `None` on malformed input.
pub(crate) fn decode_adj(s: &str) -> Option<AdjEntry> {
    // Format: "{edge_id}:{neighbor_id}:{rel_type}"
    // rel_type may contain colons, so split only on the first two colons.
    let mut parts = s.splitn(3, ':');
    let edge_raw = parts.next()?;
    let neighbor_raw = parts.next()?;
    let rel_type = parts.next()?.to_string();
    let edge_id = EdgeId(edge_raw.parse().ok()?);
    let neighbor_id = NodeId(neighbor_raw.parse().ok()?);
    Some(AdjEntry {
        edge_id,
        neighbor_id,
        rel_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_adj_round_trip() {
        let encoded = encode_adj(EdgeId(10), NodeId(20), "KNOWS");
        assert_eq!(encoded, "10:20:KNOWS");
        let decoded = decode_adj(&encoded).expect("should decode");
        assert_eq!(decoded.edge_id, EdgeId(10));
        assert_eq!(decoded.neighbor_id, NodeId(20));
        assert_eq!(decoded.rel_type, "KNOWS");
    }

    #[test]
    fn encode_adj_with_zero_ids() {
        let encoded = encode_adj(EdgeId(0), NodeId(0), "REL");
        assert_eq!(encoded, "0:0:REL");
        let decoded = decode_adj(&encoded).unwrap();
        assert_eq!(decoded.edge_id, EdgeId(0));
        assert_eq!(decoded.neighbor_id, NodeId(0));
        assert_eq!(decoded.rel_type, "REL");
    }

    #[test]
    fn encode_adj_with_large_ids() {
        let encoded = encode_adj(EdgeId(u64::MAX), NodeId(u64::MAX), "TYPE");
        let decoded = decode_adj(&encoded).unwrap();
        assert_eq!(decoded.edge_id, EdgeId(u64::MAX));
        assert_eq!(decoded.neighbor_id, NodeId(u64::MAX));
        assert_eq!(decoded.rel_type, "TYPE");
    }

    #[test]
    fn decode_adj_rel_type_with_colons() {
        // rel_type may contain colons; only the first two colons are split on.
        let encoded = "5:10:HAS:COLON:IN:NAME";
        let decoded = decode_adj(encoded).unwrap();
        assert_eq!(decoded.edge_id, EdgeId(5));
        assert_eq!(decoded.neighbor_id, NodeId(10));
        assert_eq!(decoded.rel_type, "HAS:COLON:IN:NAME");
    }

    #[test]
    fn decode_adj_malformed_input_returns_none() {
        assert!(decode_adj("").is_none());
        assert!(decode_adj("single").is_none());
        assert!(decode_adj("one:two").is_none()); // only 2 parts, no rel_type
        assert!(decode_adj("notanumber:20:REL").is_none());
        assert!(decode_adj("10:notanumber:REL").is_none());
    }
}
