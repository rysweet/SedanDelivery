using System.Text.Json;
using System.Text.Json.Serialization;

namespace SedanDelivery.Api.Models;

/// <summary>
/// Represents a directed edge (relationship) in the property graph.
/// </summary>
public record GraphEdge(
    [property: JsonPropertyName("id")] string Id,
    [property: JsonPropertyName("type")] string Type,
    [property: JsonPropertyName("source")] string Source,
    [property: JsonPropertyName("target")] string Target,
    [property: JsonPropertyName("properties")] Dictionary<string, JsonElement> Properties
);
