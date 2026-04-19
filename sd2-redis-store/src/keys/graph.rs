// Per-graph data key formatters.

pub(crate) fn key_node(g: &str, id: u64) -> String {
    format!("G:{{{g}}}:node:{id}")
}

pub(crate) fn key_edge(g: &str, id: u64) -> String {
    format!("G:{{{g}}}:edge:{id}")
}

pub(crate) fn key_node_seq(g: &str) -> String {
    format!("G:{{{g}}}:node_seq")
}

pub(crate) fn key_edge_seq(g: &str) -> String {
    format!("G:{{{g}}}:edge_seq")
}

pub(crate) fn key_label(g: &str, label: &str) -> String {
    format!("G:{{{g}}}:label:{label}")
}

pub(crate) fn key_reltype(g: &str, rel_type: &str) -> String {
    format!("G:{{{g}}}:reltype:{rel_type}")
}

pub(crate) fn key_out(g: &str, node_id: u64) -> String {
    format!("G:{{{g}}}:out:{node_id}")
}

pub(crate) fn key_in(g: &str, node_id: u64) -> String {
    format!("G:{{{g}}}:in:{node_id}")
}

pub(crate) fn key_nodes_set(g: &str) -> String {
    format!("G:{{{g}}}:nodes")
}

pub(crate) fn key_edges_set(g: &str) -> String {
    format!("G:{{{g}}}:edges")
}

pub(crate) fn key_indexes(g: &str) -> String {
    format!("G:{{{g}}}:indexes")
}

/// Temporary marker set while `delete_graph` cleanup is in progress.
/// Public reads treat a graph with this marker as already deleted, but the
/// marker keeps the name reserved so cleanup can be retried after partial
/// failures.
pub(crate) fn key_graph_deleting(g: &str) -> String {
    format!("G:{{{g}}}:_deleting")
}

/// Registry of label names seen in a graph. Used for cluster-safe label
/// enumeration without SCAN.
pub(crate) fn key_label_names(g: &str) -> String {
    format!("G:{{{g}}}:_labels")
}

/// Registry of relationship type names seen in a graph. Used for cluster-safe
/// reltype enumeration without SCAN.
pub(crate) fn key_reltype_names(g: &str) -> String {
    format!("G:{{{g}}}:_reltypes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_node_format() {
        assert_eq!(key_node("mydb", 42), "G:{mydb}:node:42");
        assert_eq!(key_node("test", 0), "G:{test}:node:0");
    }

    #[test]
    fn key_edge_format() {
        assert_eq!(key_edge("mydb", 99), "G:{mydb}:edge:99");
    }

    #[test]
    fn key_node_seq_format() {
        assert_eq!(key_node_seq("g1"), "G:{g1}:node_seq");
    }

    #[test]
    fn key_edge_seq_format() {
        assert_eq!(key_edge_seq("g1"), "G:{g1}:edge_seq");
    }

    #[test]
    fn key_label_format() {
        assert_eq!(key_label("mydb", "Person"), "G:{mydb}:label:Person");
    }

    #[test]
    fn key_reltype_format() {
        assert_eq!(key_reltype("mydb", "KNOWS"), "G:{mydb}:reltype:KNOWS");
    }

    #[test]
    fn key_out_format() {
        assert_eq!(key_out("mydb", 7), "G:{mydb}:out:7");
    }

    #[test]
    fn key_in_format() {
        assert_eq!(key_in("mydb", 7), "G:{mydb}:in:7");
    }

    #[test]
    fn key_nodes_set_format() {
        assert_eq!(key_nodes_set("g"), "G:{g}:nodes");
    }

    #[test]
    fn key_edges_set_format() {
        assert_eq!(key_edges_set("g"), "G:{g}:edges");
    }

    #[test]
    fn key_indexes_format() {
        assert_eq!(key_indexes("db"), "G:{db}:indexes");
    }

    #[test]
    fn key_graph_deleting_format() {
        assert_eq!(key_graph_deleting("db"), "G:{db}:_deleting");
    }

    #[test]
    fn key_label_names_format() {
        assert_eq!(key_label_names("db"), "G:{db}:_labels");
    }

    #[test]
    fn key_reltype_names_format() {
        assert_eq!(key_reltype_names("db"), "G:{db}:_reltypes");
    }
}
