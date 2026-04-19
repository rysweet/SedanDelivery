//! Changelog stream key constants and helpers.

/// Legacy constant kept for backward compatibility (e.g. tests, DLQ stream).
pub const CHANGELOG_STREAM: &str = "__sd2:changelog";

/// Return the per-graph changelog stream key.
///
/// The `{graphname}` hash tag ensures the stream lands on the same Redis
/// Cluster slot as the graph data, avoiding cross-slot issues.
///
/// For graph-level operations without a specific graph (e.g. list_graphs),
/// use [`CHANGELOG_STREAM`] directly.
pub fn changelog_stream_for_graph(graph: &str) -> String {
    format!("__sd2:changelog:{{{graph}}}")
}
