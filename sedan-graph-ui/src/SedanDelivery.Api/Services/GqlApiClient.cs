using System.Net.Http.Json;
using System.Text.Json;
using SedanDelivery.Api.Models;

namespace SedanDelivery.Api.Services;

/// <summary>
/// Typed HTTP client that wraps all endpoints exposed by the Rust sd2-api service.
/// All methods are virtual so that they can be mocked in tests.
/// All methods throw <see cref="HttpRequestException"/> when the upstream returns a
/// non-success status code, including the upstream error body in the message.
/// </summary>
public class GqlApiClient : IGqlApiClient
{
    private readonly HttpClient _http;
    private static readonly JsonSerializerOptions _json = new(JsonSerializerDefaults.Web);

    public GqlApiClient(HttpClient http)
    {
        _http = http;
    }

    // ── Graphs ────────────────────────────────────────────────────────────────

    /// <summary>Returns all graphs known to the sd2-api.</summary>
    public virtual async Task<List<GraphInfo>> GetGraphsAsync(CancellationToken ct = default)
    {
        var response = await _http.GetAsync("/graphs", ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<List<GraphInfo>>(_json, ct))!;
    }

    /// <summary>Creates a new named graph. Returns 201 Created on success.</summary>
    public virtual async Task<GraphInfo> CreateGraphAsync(CreateGraphRequest request, CancellationToken ct = default)
    {
        var response = await _http.PostAsJsonAsync("/graphs", request, _json, ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<GraphInfo>(_json, ct))!;
    }

    /// <summary>Deletes a graph by name.</summary>
    public virtual async Task DeleteGraphAsync(string name, CancellationToken ct = default)
    {
        var response = await _http.DeleteAsync($"/graphs/{Uri.EscapeDataString(name)}", ct);
        await EnsureSuccessAsync(response);
    }

    /// <summary>Returns statistics for a specific graph.</summary>
    public virtual async Task<GraphStats> GetGraphStatsAsync(string name, CancellationToken ct = default)
    {
        var response = await _http.GetAsync($"/graphs/{Uri.EscapeDataString(name)}/stats", ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<GraphStats>(_json, ct))!;
    }

    // ── Queries / Mutations ───────────────────────────────────────────────────

    /// <summary>Executes a read-only Cypher query against the named graph.</summary>
    public virtual async Task<QueryResponse> ExecuteQueryAsync(
        string graphName,
        QueryRequest request,
        CancellationToken ct = default)
    {
        var response = await _http.PostAsJsonAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/query", request, _json, ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<QueryResponse>(_json, ct))!;
    }

    /// <summary>Executes a write Cypher statement against the named graph.</summary>
    public virtual async Task<MutationResponse> ExecuteMutationAsync(
        string graphName,
        MutationRequest request,
        CancellationToken ct = default)
    {
        var response = await _http.PostAsJsonAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/mutate", request, _json, ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<MutationResponse>(_json, ct))!;
    }

    // ── Nodes ─────────────────────────────────────────────────────────────────

    /// <summary>Returns all nodes in the named graph.</summary>
    public virtual async Task<List<GraphNode>> GetNodesAsync(string graphName, CancellationToken ct = default)
    {
        var response = await _http.GetAsync($"/graphs/{Uri.EscapeDataString(graphName)}/nodes", ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<List<GraphNode>>(_json, ct))!;
    }

    /// <summary>Creates a new node in the named graph.</summary>
    public virtual async Task<GraphNode> CreateNodeAsync(
        string graphName,
        CreateNodeRequest request,
        CancellationToken ct = default)
    {
        var response = await _http.PostAsJsonAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/nodes", request, _json, ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<GraphNode>(_json, ct))!;
    }

    /// <summary>Returns a single node by ID.</summary>
    public virtual async Task<GraphNode> GetNodeAsync(string graphName, string nodeId, CancellationToken ct = default)
    {
        var response = await _http.GetAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/nodes/{Uri.EscapeDataString(nodeId)}", ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<GraphNode>(_json, ct))!;
    }

    /// <summary>Replaces a node's properties.</summary>
    public virtual async Task<GraphNode> UpdateNodeAsync(
        string graphName,
        string nodeId,
        CreateNodeRequest request,
        CancellationToken ct = default)
    {
        var response = await _http.PutAsJsonAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/nodes/{Uri.EscapeDataString(nodeId)}",
            request, _json, ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<GraphNode>(_json, ct))!;
    }

    /// <summary>Deletes a node by ID.</summary>
    public virtual async Task DeleteNodeAsync(string graphName, string nodeId, CancellationToken ct = default)
    {
        var response = await _http.DeleteAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/nodes/{Uri.EscapeDataString(nodeId)}", ct);
        await EnsureSuccessAsync(response);
    }

    // ── Edges ─────────────────────────────────────────────────────────────────

    /// <summary>Returns all edges in the named graph.</summary>
    public virtual async Task<List<GraphEdge>> GetEdgesAsync(string graphName, CancellationToken ct = default)
    {
        var response = await _http.GetAsync($"/graphs/{Uri.EscapeDataString(graphName)}/edges", ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<List<GraphEdge>>(_json, ct))!;
    }

    /// <summary>Creates a new edge in the named graph.</summary>
    public virtual async Task<GraphEdge> CreateEdgeAsync(
        string graphName,
        CreateEdgeRequest request,
        CancellationToken ct = default)
    {
        var response = await _http.PostAsJsonAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/edges", request, _json, ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<GraphEdge>(_json, ct))!;
    }

    /// <summary>Returns a single edge by ID.</summary>
    public virtual async Task<GraphEdge> GetEdgeAsync(string graphName, string edgeId, CancellationToken ct = default)
    {
        var response = await _http.GetAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/edges/{Uri.EscapeDataString(edgeId)}", ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<GraphEdge>(_json, ct))!;
    }

    /// <summary>Replaces an edge's properties.</summary>
    public virtual async Task<GraphEdge> UpdateEdgeAsync(
        string graphName,
        string edgeId,
        CreateEdgeRequest request,
        CancellationToken ct = default)
    {
        var response = await _http.PutAsJsonAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/edges/{Uri.EscapeDataString(edgeId)}",
            request, _json, ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<GraphEdge>(_json, ct))!;
    }

    /// <summary>Deletes an edge by ID.</summary>
    public virtual async Task DeleteEdgeAsync(string graphName, string edgeId, CancellationToken ct = default)
    {
        var response = await _http.DeleteAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/edges/{Uri.EscapeDataString(edgeId)}", ct);
        await EnsureSuccessAsync(response);
    }

    // ── Schema ────────────────────────────────────────────────────────────────

    /// <summary>Returns label and relationship-type statistics for the named graph.</summary>
    public virtual async Task<GraphSchema> GetSchemaAsync(string graphName, CancellationToken ct = default)
    {
        var response = await _http.GetAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/schema", ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<GraphSchema>(_json, ct))!;
    }

    // ── Neighbourhood ─────────────────────────────────────────────────────────

    /// <summary>Returns the neighbours of a node (connected nodes + edges).</summary>
    public virtual async Task<ExportGraph> GetNeighboursAsync(
        string graphName,
        string nodeId,
        CancellationToken ct = default)
    {
        var response = await _http.GetAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/neighbors/{Uri.EscapeDataString(nodeId)}", ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<ExportGraph>(_json, ct))!;
    }

    // ── Export / Import ───────────────────────────────────────────────────────

    /// <summary>Exports all nodes and edges of the named graph.</summary>
    public virtual async Task<ExportGraph> ExportGraphAsync(string graphName, CancellationToken ct = default)
    {
        var response = await _http.GetAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/export", ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<ExportGraph>(_json, ct))!;
    }

    /// <summary>Bulk-imports nodes and edges into the named graph.</summary>
    public virtual async Task<ImportResult> ImportGraphAsync(
        string graphName,
        ImportGraph payload,
        CancellationToken ct = default)
    {
        var response = await _http.PostAsJsonAsync(
            $"/graphs/{Uri.EscapeDataString(graphName)}/import", payload, _json, ct);
        await EnsureSuccessAsync(response);
        return (await response.Content.ReadFromJsonAsync<ImportResult>(_json, ct))!;
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// <summary>
    /// Throws an <see cref="HttpRequestException"/> that includes the upstream HTTP status
    /// and body when the response is not a success status code.
    /// </summary>
    private static async Task EnsureSuccessAsync(HttpResponseMessage response)
    {
        if (response.IsSuccessStatusCode)
        {
            return;
        }

        var body = await response.Content.ReadAsStringAsync();
        throw new HttpRequestException(
            $"sd2-api returned {(int)response.StatusCode} {response.ReasonPhrase}: {body}",
            inner: null,
            statusCode: response.StatusCode);
    }
}
