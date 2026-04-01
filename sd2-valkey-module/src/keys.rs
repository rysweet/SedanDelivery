// keys.rs -- Key schema helpers mirroring sd2-redis-store/src/keys.rs.
//
// The key format uses Redis Cluster hash tags ({graph}) so all keys for a
// given graph land in the same hash slot.  The optional key_prefix is prepended
// before the "G:" prefix, matching RedisGraphStore::kp().

/// Build a fully-prefixed key.  When `prefix` is empty this is just `G:{graph}:...`.
#[inline]
fn prefixed(prefix: &str, rest: &str) -> String {
    if prefix.is_empty() {
        rest.to_string()
    } else {
        format!("{prefix}{rest}")
    }
}

pub fn key_edge(prefix: &str, graph: &str, id: u64) -> String {
    prefixed(prefix, &format!("G:{{{graph}}}:edge:{id}"))
}

pub fn key_edge_seq(prefix: &str, graph: &str) -> String {
    prefixed(prefix, &format!("G:{{{graph}}}:edge_seq"))
}

pub fn key_out(prefix: &str, graph: &str, node_id: u64) -> String {
    prefixed(prefix, &format!("G:{{{graph}}}:out:{node_id}"))
}

pub fn key_in(prefix: &str, graph: &str, node_id: u64) -> String {
    prefixed(prefix, &format!("G:{{{graph}}}:in:{node_id}"))
}

pub fn key_edges_set(prefix: &str, graph: &str) -> String {
    prefixed(prefix, &format!("G:{{{graph}}}:edges"))
}

pub fn key_reltype(prefix: &str, graph: &str, rel_type: &str) -> String {
    prefixed(prefix, &format!("G:{{{graph}}}:reltype:{rel_type}"))
}

pub fn key_reltype_names(prefix: &str, graph: &str) -> String {
    prefixed(prefix, &format!("G:{{{graph}}}:_reltypes"))
}

// -- Adjacency encoding (mirrors sd2-redis-store/src/keys.rs) -----------------

/// Encode an adjacency set member: "{edge_id}:{neighbor_id}:{rel_type}"
pub fn encode_adj(edge_id: u64, neighbor_id: u64, rel_type: &str) -> String {
    format!("{edge_id}:{neighbor_id}:{rel_type}")
}

/// Decoded adjacency entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdjEntry {
    pub edge_id: u64,
    pub neighbor_id: u64,
    pub rel_type: String,
}

/// Decode an adjacency set member.  Returns `None` on malformed input.
/// Format: "{edge_id}:{neighbor_id}:{rel_type}" -- rel_type may contain colons,
/// so we split on only the first two colons.
pub fn decode_adj(s: &str) -> Option<AdjEntry> {
    let mut parts = s.splitn(3, ':');
    let edge_id = parts.next()?.parse().ok()?;
    let neighbor_id = parts.next()?.parse().ok()?;
    let rel_type = parts.next()?.to_string();
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
    fn key_edge_no_prefix() {
        assert_eq!(key_edge("", "mydb", 99), "G:{mydb}:edge:99");
    }

    #[test]
    fn key_edge_with_prefix() {
        assert_eq!(key_edge("sd2:", "mydb", 99), "sd2:G:{mydb}:edge:99");
    }

    #[test]
    fn key_edge_seq_format() {
        assert_eq!(key_edge_seq("", "g1"), "G:{g1}:edge_seq");
    }

    #[test]
    fn key_out_format() {
        assert_eq!(key_out("", "mydb", 7), "G:{mydb}:out:7");
    }

    #[test]
    fn key_in_format() {
        assert_eq!(key_in("", "mydb", 7), "G:{mydb}:in:7");
    }

    #[test]
    fn key_edges_set_format() {
        assert_eq!(key_edges_set("", "g"), "G:{g}:edges");
    }

    #[test]
    fn key_reltype_format() {
        assert_eq!(key_reltype("", "mydb", "KNOWS"), "G:{mydb}:reltype:KNOWS");
    }

    #[test]
    fn key_reltype_names_format() {
        assert_eq!(key_reltype_names("", "db"), "G:{db}:_reltypes");
    }

    #[test]
    fn all_keys_contain_hash_tag() {
        let g = "testgraph";
        let tag = format!("{{{g}}}");
        assert!(key_edge("", g, 1).contains(&tag));
        assert!(key_edge_seq("", g).contains(&tag));
        assert!(key_out("", g, 1).contains(&tag));
        assert!(key_in("", g, 1).contains(&tag));
        assert!(key_edges_set("", g).contains(&tag));
        assert!(key_reltype("", g, "R").contains(&tag));
        assert!(key_reltype_names("", g).contains(&tag));
    }

    #[test]
    fn encode_decode_adj_round_trip() {
        let encoded = encode_adj(10, 20, "KNOWS");
        assert_eq!(encoded, "10:20:KNOWS");
        let decoded = decode_adj(&encoded).unwrap();
        assert_eq!(decoded.edge_id, 10);
        assert_eq!(decoded.neighbor_id, 20);
        assert_eq!(decoded.rel_type, "KNOWS");
    }

    #[test]
    fn decode_adj_rel_type_with_colons() {
        let decoded = decode_adj("5:10:HAS:COLON:IN:NAME").unwrap();
        assert_eq!(decoded.edge_id, 5);
        assert_eq!(decoded.neighbor_id, 10);
        assert_eq!(decoded.rel_type, "HAS:COLON:IN:NAME");
    }

    #[test]
    fn decode_adj_malformed() {
        assert!(decode_adj("").is_none());
        assert!(decode_adj("single").is_none());
        assert!(decode_adj("one:two").is_none());
        assert!(decode_adj("notanumber:20:REL").is_none());
    }
}
