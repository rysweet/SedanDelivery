// merge/resolve.rs -- Match-vs-create conflict resolution per edge spec.

use std::collections::HashMap;

use valkey_module::{Context, ValkeyError, ValkeyValue};

use crate::keys;

use super::edge_ops::{allocate_edge_id, apply_set_props, create_edge, smembers_as_adj};
use super::props::{parse_props_obj, read_edge_props};

/// Parsed edge spec (owned strings to avoid lifetime issues with ValkeyString).
pub(super) struct EdgeSpec {
    pub src_id: u64,
    pub dst_id: u64,
    pub rel_type: String,
    pub edge_props: String,
    pub set_props: String,
}

/// Resolve a single edge spec: match an existing edge (and SET) or create.
/// Mutates `adj_cache` with newly created entries to keep subsequent specs
/// in the same batch coherent.
pub(super) fn resolve_edge(
    ctx: &Context,
    prefix: &str,
    graph: &str,
    spec: &EdgeSpec,
    adj_cache: &mut HashMap<u64, Vec<keys::AdjEntry>>,
) -> Result<ValkeyValue, ValkeyError> {
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
        Ok(ValkeyValue::Array(vec![
            ValkeyValue::Integer(eid as i64),
            ValkeyValue::SimpleString("matched".to_string()),
        ]))
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

        Ok(ValkeyValue::Array(vec![
            ValkeyValue::Integer(new_id as i64),
            ValkeyValue::SimpleString("created".to_string()),
        ]))
    }
}
