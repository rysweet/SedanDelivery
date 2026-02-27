using Microsoft.AspNetCore.Mvc;
using Microsoft.AspNetCore.RateLimiting;
using SedanDelivery.Api.Models;
using SedanDelivery.Api.Services;
using SedanDelivery.Api.Validation;

namespace SedanDelivery.Api.Controllers;

/// <summary>
/// CRUD operations for named graphs.
/// </summary>
[ApiController]
[EnableRateLimiting("query")]
[Route("api/graphs")]
[Produces("application/json")]
public sealed class GraphsController : GraphControllerBase
{
    private readonly IGqlApiClient _client;
    private readonly ILogger<GraphsController> _logger;

    public GraphsController(IGqlApiClient client, ILogger<GraphsController> logger)
    {
        _client = client;
        _logger = logger;
    }

    /// <summary>Returns all graphs.</summary>
    [HttpGet]
    [ProducesResponseType(typeof(List<GraphInfo>), StatusCodes.Status200OK)]
    public async Task<IActionResult> GetGraphs(CancellationToken ct)
    {
        var graphs = await _client.GetGraphsAsync(ct);
        return Ok(graphs);
    }

    /// <summary>Creates a new named graph.</summary>
    [HttpPost]
    [ProducesResponseType(typeof(GraphInfo), StatusCodes.Status201Created)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    public async Task<IActionResult> CreateGraph([FromBody] CreateGraphRequest request, CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(request.Name))
        {
            return InvalidName();
        }

        var graph = await _client.CreateGraphAsync(request, ct);
        return CreatedAtAction(nameof(GetGraphStats), new { name = graph.Name }, graph);
    }

    /// <summary>Deletes a graph by name.</summary>
    [HttpDelete("{name}")]
    [ProducesResponseType(StatusCodes.Status204NoContent)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public async Task<IActionResult> DeleteGraph(string name, CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name))
        {
            return InvalidName();
        }

        try
        {
            await _client.DeleteGraphAsync(name, ct);
            return NoContent();
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.NotFound)
        {
            return NotFound();
        }
    }

    /// <summary>Returns statistics for a specific graph.</summary>
    [HttpGet("{name}/stats")]
    [ProducesResponseType(typeof(GraphStats), StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public async Task<IActionResult> GetGraphStats(string name, CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name))
        {
            return InvalidName();
        }

        try
        {
            var stats = await _client.GetGraphStatsAsync(name, ct);
            return Ok(stats);
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.NotFound)
        {
            return NotFound();
        }
    }
}
