using System.Text.Json;
using System.Text.Json.Serialization;

namespace SedanDelivery.Api.Models;

/// <summary>
/// Result of a Cypher query execution.
/// </summary>
public record QueryResponse(
    [property: JsonPropertyName("columns")] string[] Columns,
    [property: JsonPropertyName("rows")] List<Dictionary<string, JsonElement>> Rows,
    [property: JsonPropertyName("elapsedMs")] long ElapsedMs
);
