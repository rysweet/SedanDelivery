// merge/mod.rs -- SD2.MERGE_EDGES command implementation.
//
// Executes the entire MERGE edge check-then-create cycle server-side in a
// single round-trip, eliminating all client-server overhead.

use std::collections::HashMap;
use valkey_module::{Context, ValkeyError, ValkeyResult, ValkeyString, ValkeyValue};

use crate::keys;

mod edge_ops;
mod props;
mod resolve;

use resolve::{resolve_edge, EdgeSpec};

/// Shorthand to create a ValkeyString from the current context.
pub(crate) fn vs(ctx: &Context, s: &str) -> ValkeyString {
    ValkeyString::create(std::ptr::NonNull::new(ctx.ctx), s)
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
        let item = resolve_edge(ctx, prefix, graph, spec, &mut adj_cache)?;
        results.push(item);
    }

    Ok(ValkeyValue::Array(results))
}
