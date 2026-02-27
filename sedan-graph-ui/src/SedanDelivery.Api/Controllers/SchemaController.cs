using Microsoft.AspNetCore.Mvc;
using SedanDelivery.Api.Models;
using SedanDelivery.Api.Services;
using SedanDelivery.Api.Validation;

namespace SedanDelivery.Api.Controllers;

/// <summary>
/// Returns schema information (label/relationship-type statistics) for a named graph,
/// and exposes the neighbours sub-graph for a node.
/// </summary>
[ApiController]
[Route("api/graphs/{name}")]
[Produces("application/json")]
public sealed class SchemaController : ControllerBase
{
    private readonly GqlApiClient _client;

    public SchemaController(GqlApiClient client)
    {
        _client = client;
    }

    /// <summary>Returns label and relationship-type statistics for the named graph.</summary>
    [HttpGet("schema")]
    [ProducesResponseType(typeof(GraphSchema), StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public async Task<IActionResult> GetSchema(string name, CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        try
        {
            return Ok(await _client.GetSchemaAsync(name, ct));
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.NotFound)
        {
            return NotFound();
        }
    }

    /// <summary>Returns the neighbourhood (connected nodes and edges) of the specified node.</summary>
    [HttpGet("neighbors/{nodeId}")]
    [ProducesResponseType(typeof(ExportGraph), StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public async Task<IActionResult> GetNeighbours(string name, string nodeId, CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        try
        {
            return Ok(await _client.GetNeighboursAsync(name, nodeId, ct));
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.NotFound)
        {
            return NotFound();
        }
    }

    private IActionResult InvalidName() =>
        Problem(
            title: "Invalid graph name",
            detail: "Graph name must match ^[a-zA-Z0-9_.\\-]{1,64}$",
            statusCode: StatusCodes.Status400BadRequest);
}
