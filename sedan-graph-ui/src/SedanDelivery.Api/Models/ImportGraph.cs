using System.Text.Json.Serialization;

namespace SedanDelivery.Api.Models;

/// <summary>
/// Graph import payload: lists of nodes and edges to bulk-insert.
/// </summary>
public record ImportGraph(
    [property: JsonPropertyName("nodes")] List<GraphNode> Nodes,
    [property: JsonPropertyName("edges")] List<GraphEdge> Edges
);

/// <summary>
/// Result of a graph import operation.
/// </summary>
public record ImportResult(
    [property: JsonPropertyName("nodesImported")] int NodesImported,
    [property: JsonPropertyName("edgesImported")] int EdgesImported
);
