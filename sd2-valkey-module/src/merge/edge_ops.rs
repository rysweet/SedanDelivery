// merge/edge_ops.rs -- Edge creation/update primitives for SD2.MERGE_EDGES.

use valkey_module::{Context, ValkeyError, ValkeyValue};

use crate::keys;

use super::props::{merge_props_json, parse_props_obj};
use super::vs;

/// Run SMEMBERS on a key and decode each member as an AdjEntry.
pub(super) fn smembers_as_adj(ctx: &Context, key: &str) -> Vec<keys::AdjEntry> {
    match ctx.call("SMEMBERS", &[key]) {
        Ok(ValkeyValue::Array(members)) => members
            .iter()
            .filter_map(|m| match m {
                ValkeyValue::SimpleString(s) | ValkeyValue::BulkString(s) => keys::decode_adj(s),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

/// Allocate a new edge ID by incrementing the edge_seq counter.
pub(super) fn allocate_edge_id(
    ctx: &Context,
    prefix: &str,
    graph: &str,
) -> Result<u64, ValkeyError> {
    let seq_key = keys::key_edge_seq(prefix, graph);
    match ctx.call("INCRBY", &[seq_key.as_str(), "1"]) {
        Ok(ValkeyValue::Integer(id)) if id > 0 => Ok(id as u64),
        Ok(ValkeyValue::Integer(_)) => Err(ValkeyError::String(
            "ERR edge_seq overflow or negative".to_string(),
        )),
        Ok(other) => Err(ValkeyError::String(format!(
            "ERR unexpected INCRBY reply: {other:?}"
        ))),
        Err(e) => Err(e),
    }
}

/// Create a new edge: write hash, update adjacency sets and indexes.
#[allow(clippy::too_many_arguments)]
pub(super) fn create_edge(
    ctx: &Context,
    prefix: &str,
    graph: &str,
    id: u64,
    src_id: u64,
    dst_id: u64,
    rel_type: &str,
    edge_props: &str,
    set_props: &str,
) -> Result<(), ValkeyError> {
    // Merge edge_props and set_props into one props JSON.
    let merged_props = merge_props_json(edge_props, set_props);

    // 1. Write edge hash: src, dst, rel_type, props
    let edge_key = keys::key_edge(prefix, graph, id);
    let edge_vs = vs(ctx, edge_key.as_str());
    let wkey = ctx.open_key_writable(&edge_vs);
    wkey.hash_set("src", vs(ctx, src_id.to_string().as_str()));
    wkey.hash_set("dst", vs(ctx, dst_id.to_string().as_str()));
    wkey.hash_set("rel_type", vs(ctx, rel_type));
    wkey.hash_set("props", vs(ctx, merged_props.as_str()));
    drop(wkey);

    // 2. Out-adjacency: SADD out:{src_id} "edge_id:dst_id:rel_type"
    let out_key = keys::key_out(prefix, graph, src_id);
    let out_member = keys::encode_adj(id, dst_id, rel_type);
    ctx.call("SADD", &[out_key.as_str(), out_member.as_str()])?;

    // 3. In-adjacency: SADD in:{dst_id} "edge_id:src_id:rel_type"
    let in_key = keys::key_in(prefix, graph, dst_id);
    let in_member = keys::encode_adj(id, src_id, rel_type);
    ctx.call("SADD", &[in_key.as_str(), in_member.as_str()])?;

    // 4. Edges set: SADD edges id
    let edges_key = keys::key_edges_set(prefix, graph);
    let id_str = id.to_string();
    ctx.call("SADD", &[edges_key.as_str(), id_str.as_str()])?;

    // 5. Reltype index: SADD reltype:{rel_type} id
    let rt_key = keys::key_reltype(prefix, graph, rel_type);
    ctx.call("SADD", &[rt_key.as_str(), id_str.as_str()])?;

    // 6. Reltype registry: SADD _reltypes rel_type
    let rtn_key = keys::key_reltype_names(prefix, graph);
    ctx.call("SADD", &[rtn_key.as_str(), rel_type])?;

    Ok(())
}

/// Apply set_props to an existing edge by merging into its props JSON.
pub(super) fn apply_set_props(
    ctx: &Context,
    prefix: &str,
    graph: &str,
    edge_id: u64,
    set_props: &str,
) -> Result<(), ValkeyError> {
    let new_props = match parse_props_obj(set_props) {
        Some(m) => m,
        None => return Ok(()),
    };

    let edge_key = keys::key_edge(prefix, graph, edge_id);

    // Read existing props.
    let existing: serde_json::Map<String, serde_json::Value> = {
        let key_vs = vs(ctx, edge_key.as_str());
        let rkey = ctx.open_key(&key_vs);
        match rkey.hash_get("props") {
            Ok(Some(val)) => {
                let s = val.to_string_lossy();
                match serde_json::from_str::<serde_json::Value>(&s) {
                    Ok(serde_json::Value::Object(m)) => m,
                    _ => serde_json::Map::new(),
                }
            }
            _ => serde_json::Map::new(),
        }
    };

    // Merge: new_props overwrite existing keys.
    let mut merged = existing;
    for (k, v) in new_props {
        merged.insert(k, v);
    }

    let merged_json =
        serde_json::to_string(&serde_json::Value::Object(merged)).unwrap_or_else(|_| "{}".into());

    // Write back.
    let key_vs = vs(ctx, edge_key.as_str());
    let wkey = ctx.open_key_writable(&key_vs);
    wkey.hash_set("props", vs(ctx, merged_json.as_str()));

    Ok(())
}
