using Microsoft.AspNetCore.Mvc;
using Microsoft.AspNetCore.RateLimiting;
using SedanDelivery.Api.Models;
using SedanDelivery.Api.Services;
using SedanDelivery.Api.Validation;

namespace SedanDelivery.Api.Controllers;

/// <summary>
/// Graph export and import operations.
/// </summary>
[ApiController]
[EnableRateLimiting("query")]
[Route("api/graphs/{name}")]
[Produces("application/json")]
public sealed class ExportController : GraphControllerBase
{
    private readonly IGqlApiClient _client;

    public ExportController(IGqlApiClient client)
    {
        _client = client;
    }

    /// <summary>Exports all nodes and edges of the named graph as JSON.</summary>
    [HttpGet("export")]
    [ProducesResponseType(typeof(ExportGraph), StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public async Task<IActionResult> ExportGraph(string name, CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        try
        {
            return Ok(await _client.ExportGraphAsync(name, ct));
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.NotFound)
        {
            return NotFound();
        }
    }

    /// <summary>Bulk-imports nodes and edges into the named graph.</summary>
    [HttpPost("import")]
    [ProducesResponseType(typeof(ImportResult), StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public async Task<IActionResult> ImportGraph(
        string name,
        [FromBody] ImportGraph payload,
        CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        try
        {
            return Ok(await _client.ImportGraphAsync(name, payload, ct));
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.NotFound)
        {
            return NotFound();
        }
    }
}
