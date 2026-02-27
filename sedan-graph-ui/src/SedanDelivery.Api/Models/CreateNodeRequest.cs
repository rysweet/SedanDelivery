using System.ComponentModel.DataAnnotations;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace SedanDelivery.Api.Models;

/// <summary>
/// Request body for creating a new node.
/// </summary>
public record CreateNodeRequest(
    [property: JsonPropertyName("labels")]
    [Required]
    string[] Labels,

    [property: JsonPropertyName("properties")]
    Dictionary<string, JsonElement> Properties
);
