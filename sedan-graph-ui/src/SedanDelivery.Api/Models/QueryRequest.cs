using System.ComponentModel.DataAnnotations;
using System.Text.Json.Serialization;

namespace SedanDelivery.Api.Models;

/// <summary>
/// Request body for executing a read-only Cypher query.
/// </summary>
public record QueryRequest(
    [property: JsonPropertyName("cypher")]
    [Required(AllowEmptyStrings = false, ErrorMessage = "cypher is required")]
    string Cypher
);
