using System.Text.Json.Serialization;

namespace SedanDelivery.Api.Models;

/// <summary>
/// Full graph export: all nodes and edges.
/// </summary>
public record ExportGraph(
    [property: JsonPropertyName("nodes")] List<GraphNode> Nodes,
    [property: JsonPropertyName("edges")] List<GraphEdge> Edges
);
