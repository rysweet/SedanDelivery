using SedanDelivery.Api.Models;

namespace SedanDelivery.Api.Services;

/// <summary>
/// Abstraction over the Rust gql-api HTTP client.
/// Enables mocking in unit tests without needing a live HTTP server.
/// </summary>
public interface IGqlApiClient
{
    // ── Graphs ────────────────────────────────────────────────────────────────
    Task<List<GraphInfo>> GetGraphsAsync(CancellationToken ct = default);
    Task<GraphInfo> CreateGraphAsync(CreateGraphRequest request, CancellationToken ct = default);
    Task DeleteGraphAsync(string name, CancellationToken ct = default);
    Task<GraphStats> GetGraphStatsAsync(string name, CancellationToken ct = default);

    // ── Queries / Mutations ───────────────────────────────────────────────────
    Task<QueryResponse> ExecuteQueryAsync(string graphName, QueryRequest request, CancellationToken ct = default);
    Task<MutationResponse> ExecuteMutationAsync(string graphName, MutationRequest request, CancellationToken ct = default);

    // ── Nodes ─────────────────────────────────────────────────────────────────
    Task<List<GraphNode>> GetNodesAsync(string graphName, CancellationToken ct = default);
    Task<GraphNode> CreateNodeAsync(string graphName, CreateNodeRequest request, CancellationToken ct = default);
    Task<GraphNode> GetNodeAsync(string graphName, string nodeId, CancellationToken ct = default);
    Task<GraphNode> UpdateNodeAsync(string graphName, string nodeId, CreateNodeRequest request, CancellationToken ct = default);
    Task DeleteNodeAsync(string graphName, string nodeId, CancellationToken ct = default);

    // ── Edges ─────────────────────────────────────────────────────────────────
    Task<List<GraphEdge>> GetEdgesAsync(string graphName, CancellationToken ct = default);
    Task<GraphEdge> CreateEdgeAsync(string graphName, CreateEdgeRequest request, CancellationToken ct = default);
    Task<GraphEdge> GetEdgeAsync(string graphName, string edgeId, CancellationToken ct = default);
    Task<GraphEdge> UpdateEdgeAsync(string graphName, string edgeId, CreateEdgeRequest request, CancellationToken ct = default);
    Task DeleteEdgeAsync(string graphName, string edgeId, CancellationToken ct = default);

    // ── Schema ────────────────────────────────────────────────────────────────
    Task<GraphSchema> GetSchemaAsync(string graphName, CancellationToken ct = default);

    // ── Neighbourhood ─────────────────────────────────────────────────────────
    Task<ExportGraph> GetNeighboursAsync(string graphName, string nodeId, CancellationToken ct = default);

    // ── Export / Import ───────────────────────────────────────────────────────
    Task<ExportGraph> ExportGraphAsync(string graphName, CancellationToken ct = default);
    Task<ImportResult> ImportGraphAsync(string graphName, ImportGraph payload, CancellationToken ct = default);
}
