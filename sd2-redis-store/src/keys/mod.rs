// keys — Redis key formatting and adjacency encoding helpers.
//
// Decomposed into focused submodules:
//   - `adj`    : adjacency set member encode/decode
//   - `graph`  : per-graph data key formatters (nodes, edges, labels, sets)
//   - `index`  : property-index keys and value-to-key encoding
//   - `locks`  : distributed write-lock keys and the global graph registry

mod adj;
mod graph;
mod index;
mod locks;

pub(crate) use adj::{decode_adj, encode_adj};
pub(crate) use graph::{
    key_edge, key_edge_seq, key_edges_set, key_graph_deleting, key_in, key_indexes, key_label,
    key_label_names, key_node, key_node_seq, key_nodes_set, key_out, key_reltype,
    key_reltype_names,
};
pub(crate) use index::{
    key_propidx, key_propidx_manifest, key_propidx_prefix, value_to_index_key, IDX_SEP,
};
pub(crate) use locks::{
    key_batch_edge_write_lock, key_batch_node_write_lock, key_edge_id_write_lock,
    key_edge_write_lock, key_graph_lock, key_graph_lock_fence, key_node_write_lock, KEY_GRAPHS,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Cross-module guarantee: every per-graph key embeds the `{g}` hash tag so
    /// Redis Cluster routes related keys to the same slot.
    #[test]
    fn all_graph_keys_contain_hash_tag() {
        let g = "testgraph";
        let tag = format!("{{{g}}}");
        assert!(key_node(g, 1).contains(&tag));
        assert!(key_edge(g, 1).contains(&tag));
        assert!(key_node_seq(g).contains(&tag));
        assert!(key_edge_seq(g).contains(&tag));
        assert!(key_label(g, "L").contains(&tag));
        assert!(key_reltype(g, "R").contains(&tag));
        assert!(key_out(g, 1).contains(&tag));
        assert!(key_in(g, 1).contains(&tag));
        assert!(key_nodes_set(g).contains(&tag));
        assert!(key_edges_set(g).contains(&tag));
        assert!(key_indexes(g).contains(&tag));
        assert!(key_graph_lock(g).contains(&tag));
        assert!(key_node_write_lock(g, 1).contains(&tag));
        assert!(key_edge_id_write_lock(g, 1).contains(&tag));
        assert!(key_edge_write_lock(g, 1, 2, "R").contains(&tag));
        assert!(key_batch_node_write_lock(g).contains(&tag));
        assert!(key_batch_edge_write_lock(g).contains(&tag));
        assert!(key_graph_deleting(g).contains(&tag));
        assert!(key_label_names(g).contains(&tag));
        assert!(key_reltype_names(g).contains(&tag));
        assert!(key_propidx(g, "L", "p", "v").contains(&tag));
        assert!(key_propidx_manifest(g).contains(&tag));
    }
}
