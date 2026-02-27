using System.Text.Json.Serialization;

namespace SedanDelivery.Api.Models;

/// <summary>
/// Summary statistics returned after a mutating Cypher statement.
/// </summary>
public record MutationResponse(
    [property: JsonPropertyName("nodesCreated")] int NodesCreated,
    [property: JsonPropertyName("nodesDeleted")] int NodesDeleted,
    [property: JsonPropertyName("edgesCreated")] int EdgesCreated,
    [property: JsonPropertyName("edgesDeleted")] int EdgesDeleted,
    [property: JsonPropertyName("propertiesSet")] int PropertiesSet
);
