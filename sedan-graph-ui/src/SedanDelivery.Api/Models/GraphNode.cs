using System.Text.Json;
using System.Text.Json.Serialization;

namespace SedanDelivery.Api.Models;

/// <summary>
/// Represents a node in the property graph.
/// </summary>
public record GraphNode(
    [property: JsonPropertyName("id")] string Id,
    [property: JsonPropertyName("labels")] string[] Labels,
    [property: JsonPropertyName("properties")] Dictionary<string, JsonElement> Properties
);
