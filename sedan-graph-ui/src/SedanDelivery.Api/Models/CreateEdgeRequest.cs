using System.ComponentModel.DataAnnotations;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace SedanDelivery.Api.Models;

/// <summary>
/// Request body for creating a new edge (relationship).
/// </summary>
public record CreateEdgeRequest(
    [property: JsonPropertyName("source")]
    [Required(AllowEmptyStrings = false)]
    string Source,

    [property: JsonPropertyName("target")]
    [Required(AllowEmptyStrings = false)]
    string Target,

    [property: JsonPropertyName("type")]
    [Required(AllowEmptyStrings = false)]
    string Type,

    [property: JsonPropertyName("properties")]
    Dictionary<string, JsonElement> Properties
);
