using System.Text.Json.Serialization;

namespace SedanDelivery.Api.Models;

/// <summary>
/// Summary information about a graph stored in the sd2-api.
/// </summary>
public record GraphInfo(
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("nodeCount")] long NodeCount,
    [property: JsonPropertyName("edgeCount")] long EdgeCount
);

/// <summary>
/// Request body for creating a new named graph.
/// </summary>
public record CreateGraphRequest(
    [property: JsonPropertyName("name")]
    [System.ComponentModel.DataAnnotations.Required(AllowEmptyStrings = false)]
    string Name
);

/// <summary>
/// Detailed statistics for a specific graph.
/// </summary>
public record GraphStats(
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("nodeCount")] long NodeCount,
    [property: JsonPropertyName("edgeCount")] long EdgeCount,
    [property: JsonPropertyName("labelCounts")] Dictionary<string, long> LabelCounts,
    [property: JsonPropertyName("relTypeCounts")] Dictionary<string, long> RelTypeCounts
);
