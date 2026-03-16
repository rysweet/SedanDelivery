using System.ComponentModel.DataAnnotations;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace SedanDelivery.Api.Models;

/// <summary>
/// Request body for executing a write (mutating) Cypher statement.
/// </summary>
public record MutationRequest(
    [property: JsonPropertyName("cypher")]
    [Required(AllowEmptyStrings = false, ErrorMessage = "cypher is required")]
    string Cypher,

    [property: JsonPropertyName("params")]
    IReadOnlyDictionary<string, JsonElement>? Params = null
);
