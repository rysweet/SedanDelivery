// merge.rs -- SD2.MERGE_EDGES command implementation.
//
// Executes the entire MERGE edge check-then-create cycle server-side in a
// single round-trip, eliminating all client-server overhead.

use std::collections::HashMap;
use valkey_module::{Context, ValkeyError, ValkeyResult, ValkeyString, ValkeyValue};

use crate::keys;

/// Parsed edge spec (owned strings to avoid lifetime issues with ValkeyString).
struct EdgeSpec {
    src_id: u64,
    dst_id: u64,
    rel_type: String,
    edge_props: String,
    set_props: String,
}

/// SD2.MERGE_EDGES handler.
///
/// Syntax:
/// ```text
/// SD2.MERGE_EDGES {graph} {key_prefix} {num_edges}
///   {src_id} {dst_id} {rel_type} {edge_props_json} {set_props_json}
///   ...
/// ```
///
/// Returns: Array of [edge_id, "created"|"matched"] per input edge.
pub fn merge_edges(ctx: &Context, args: Vec<ValkeyString>) -> ValkeyResult {
    // args[0] = command name
    if args.len() < 4 {
        return Err(ValkeyError::WrongArity);
    }

    let graph = args[1].to_string_lossy();
    let key_prefix = args[2].to_string_lossy();
    let num_edges: usize = args[3]
        .to_string_lossy()
        .parse()
        .map_err(|_| ValkeyError::String("ERR invalid num_edges".to_string()))?;

    let expected_args = 4 + num_edges * 5;
    if args.len() != expected_args {
        return Err(ValkeyError::String(format!(
            "ERR expected {} args (4 header + {} * 5 per edge), got {}",
            expected_args,
            num_edges,
            args.len()
        )));
    }

    // Parse edge specs into owned data.
    let mut edge_specs = Vec::with_capacity(num_edges);
    for i in 0..num_edges {
        let base = 4 + i * 5;
        let src_id: u64 = args[base]
            .to_string_lossy()
            .parse()
            .map_err(|_| ValkeyError::String(format!("ERR invalid src_id at edge {i}")))?;
        let dst_id: u64 = args[base + 1]
            .to_string_lossy()
            .parse()
            .map_err(|_| ValkeyError::String(format!("ERR invalid dst_id at edge {i}")))?;
        edge_specs.push(EdgeSpec {
            src_id,
            dst_id,
            rel_type: args[base + 2].to_string_lossy(),
            edge_props: args[base + 3].to_string_lossy(),
            set_props: args[base + 4].to_string_lossy(),
        });
    }

    let prefix = key_prefix.as_str();
    let graph = graph.as_str();

    // Cache of adjacency sets: out:{src_id} -> Vec<AdjEntry>.
    // Reuse across edges sharing the same src_id to avoid re-fetching.
    let mut adj_cache: HashMap<u64, Vec<keys::AdjEntry>> = HashMap::new();

    let mut results: Vec<ValkeyValue> = Vec::with_capacity(num_edges);

    for spec in &edge_specs {
        // Step 1: Get outgoing adjacency entries for src_id.
        let adj_entries = if let Some(cached) = adj_cache.get(&spec.src_id) {
            cached.clone()
        } else {
            let out_key = keys::key_out(prefix, graph, spec.src_id);
            let entries = smembers_as_adj(ctx, &out_key);
            adj_cache.insert(spec.src_id, entries.clone());
            entries
        };

        // Step 2: Filter candidates matching dst_id and rel_type.
        let candidates: Vec<&keys::AdjEntry> = adj_entries
            .iter()
            .filter(|e| e.neighbor_id == spec.dst_id && e.rel_type == spec.rel_type)
            .collect();

        // Step 3: Check edge properties if needed.
        let match_props: Option<serde_json::Map<String, serde_json::Value>> =
            parse_props_obj(&spec.edge_props);

        let mut matched_edge_id: Option<u64> = None;

        if let Some(ref required) = match_props {
            // Need to check each candidate's properties.
            for candidate in &candidates {
                let edge_key = keys::key_edge(prefix, graph, candidate.edge_id);
                if let Some(stored) = read_edge_props(ctx, &edge_key) {
                    if required.iter().all(|(k, v)| stored.get(k) == Some(v)) {
                        matched_edge_id = Some(candidate.edge_id);
                        break;
                    }
                }
            }
        } else if !candidates.is_empty() {
            // No property matching required; take the first candidate.
            matched_edge_id = Some(candidates[0].edge_id);
        }

        if let Some(eid) = matched_edge_id {
            // Matched -- apply set_props if any.
            apply_set_props(ctx, prefix, graph, eid, &spec.set_props)?;
            results.push(ValkeyValue::Array(vec![
                ValkeyValue::Integer(eid as i64),
                ValkeyValue::SimpleString("matched".to_string()),
            ]));
        } else {
            // Create new edge.
            let new_id = allocate_edge_id(ctx, prefix, graph)?;
            create_edge(
                ctx,
                prefix,
                graph,
                new_id,
                spec.src_id,
                spec.dst_id,
                &spec.rel_type,
                &spec.edge_props,
                &spec.set_props,
            )?;

            // Update adj_cache so subsequent edges with same src_id see the new edge.
            adj_cache
                .entry(spec.src_id)
                .or_default()
                .push(keys::AdjEntry {
                    edge_id: new_id,
                    neighbor_id: spec.dst_id,
                    rel_type: spec.rel_type.clone(),
                });

            results.push(ValkeyValue::Array(vec![
                ValkeyValue::Integer(new_id as i64),
                ValkeyValue::SimpleString("created".to_string()),
            ]));
        }
    }

    Ok(ValkeyValue::Array(results))
}

// -- Helper functions ---------------------------------------------------------

/// Shorthand to create a ValkeyString from the current context.
fn vs(ctx: &Context, s: &str) -> ValkeyString {
    ValkeyString::create(std::ptr::NonNull::new(ctx.ctx), s)
}

/// Parse a JSON string into a non-empty Map, or None.
fn parse_props_obj(s: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    if s.is_empty() || s == "{}" {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(s) {
        Ok(serde_json::Value::Object(m)) if !m.is_empty() => Some(m),
        _ => None,
    }
}

/// Run SMEMBERS on a key and decode each member as an AdjEntry.
fn smembers_as_adj(ctx: &Context, key: &str) -> Vec<keys::AdjEntry> {
    match ctx.call("SMEMBERS", &[key]) {
        Ok(ValkeyValue::Array(members)) => members
            .iter()
            .filter_map(|m| match m {
                ValkeyValue::SimpleString(s) | ValkeyValue::BulkString(s) => {
                    keys::decode_adj(s)
                }
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

/// Read the "props" field from an edge hash and parse as JSON object.
fn read_edge_props(
    ctx: &Context,
    edge_key: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let key_vs = vs(ctx, edge_key);
    let rkey = ctx.open_key(&key_vs);
    match rkey.hash_get("props") {
        Ok(Some(val)) => {
            let s = val.to_string_lossy();
            match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(serde_json::Value::Object(m)) => Some(m),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Allocate a new edge ID by incrementing the edge_seq counter.
fn allocate_edge_id(ctx: &Context, prefix: &str, graph: &str) -> Result<u64, ValkeyError> {
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
fn create_edge(
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
fn apply_set_props(
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
    wkey.hash_set(
        "props",
        vs(ctx, merged_json.as_str()),
    );

    Ok(())
}

/// Merge edge_props (MERGE match properties) and set_props into a single JSON object.
/// set_props override edge_props on key collision.
fn merge_props_json(edge_props: &str, set_props: &str) -> String {
    let mut merged = serde_json::Map::new();

    if let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(edge_props)
    {
        for (k, v) in m {
            merged.insert(k, v);
        }
    }

    if let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(set_props)
    {
        for (k, v) in m {
            merged.insert(k, v);
        }
    }

    if merged.is_empty() {
        "{}".to_string()
    } else {
        serde_json::to_string(&serde_json::Value::Object(merged))
            .unwrap_or_else(|_| "{}".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_props_both_empty() {
        assert_eq!(merge_props_json("", ""), "{}");
        assert_eq!(merge_props_json("{}", "{}"), "{}");
    }

    #[test]
    fn merge_props_only_edge() {
        let result = merge_props_json(r#"{"a":1}"#, "");
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn merge_props_only_set() {
        let result = merge_props_json("", r#"{"b":2}"#);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["b"], 2);
    }

    #[test]
    fn merge_props_set_overrides_edge() {
        let result = merge_props_json(r#"{"x":1,"y":2}"#, r#"{"x":99,"z":3}"#);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["x"], 99);
        assert_eq!(parsed["y"], 2);
        assert_eq!(parsed["z"], 3);
    }

    #[test]
    fn merge_props_invalid_json_treated_as_empty() {
        assert_eq!(merge_props_json("not json", "also not json"), "{}");
    }

    #[test]
    fn parse_props_obj_empty() {
        assert!(parse_props_obj("").is_none());
        assert!(parse_props_obj("{}").is_none());
    }

    #[test]
    fn parse_props_obj_valid() {
        let m = parse_props_obj(r#"{"key":"val"}"#).unwrap();
        assert_eq!(m.get("key").unwrap(), "val");
    }

    #[test]
    fn parse_props_obj_invalid_json() {
        assert!(parse_props_obj("not json").is_none());
    }

    #[test]
    fn parse_props_obj_array_returns_none() {
        assert!(parse_props_obj("[1,2,3]").is_none());
    }
}
