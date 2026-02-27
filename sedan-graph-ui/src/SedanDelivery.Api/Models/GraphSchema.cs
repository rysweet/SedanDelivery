using System.Text.Json.Serialization;

namespace SedanDelivery.Api.Models;

/// <summary>
/// Schema summary for a graph: label counts and relationship type counts.
/// </summary>
public record GraphSchema(
    [property: JsonPropertyName("labels")] LabelInfo[] Labels,
    [property: JsonPropertyName("relationshipTypes")] RelTypeInfo[] RelationshipTypes
);

/// <summary>
/// A node label with its occurrence count.
/// </summary>
public record LabelInfo(
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("count")] int Count
);

/// <summary>
/// A relationship type with its occurrence count.
/// </summary>
public record RelTypeInfo(
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("count")] int Count
);
