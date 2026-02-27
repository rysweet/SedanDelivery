using Microsoft.AspNetCore.Mvc;
using SedanDelivery.Api.Models;
using SedanDelivery.Api.Services;
using SedanDelivery.Api.Validation;

namespace SedanDelivery.Api.Controllers;

/// <summary>
/// CRUD operations for edges within a named graph.
/// </summary>
[ApiController]
[Route("api/graphs/{name}/edges")]
[Produces("application/json")]
public sealed class EdgesController : ControllerBase
{
    private readonly GqlApiClient _client;

    public EdgesController(GqlApiClient client)
    {
        _client = client;
    }

    /// <summary>Returns all edges in the graph.</summary>
    [HttpGet]
    [ProducesResponseType(typeof(List<GraphEdge>), StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    public async Task<IActionResult> GetEdges(string name, CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        return Ok(await _client.GetEdgesAsync(name, ct));
    }

    /// <summary>Creates a new edge in the graph.</summary>
    [HttpPost]
    [ProducesResponseType(typeof(GraphEdge), StatusCodes.Status201Created)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    public async Task<IActionResult> CreateEdge(
        string name,
        [FromBody] CreateEdgeRequest request,
        CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        var edge = await _client.CreateEdgeAsync(name, request, ct);
        return CreatedAtAction(nameof(GetEdge), new { name, id = edge.Id }, edge);
    }

    /// <summary>Returns a single edge by ID.</summary>
    [HttpGet("{id}")]
    [ProducesResponseType(typeof(GraphEdge), StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public async Task<IActionResult> GetEdge(string name, string id, CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        try
        {
            return Ok(await _client.GetEdgeAsync(name, id, ct));
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.NotFound)
        {
            return NotFound();
        }
    }

    /// <summary>Replaces an edge's properties.</summary>
    [HttpPut("{id}")]
    [ProducesResponseType(typeof(GraphEdge), StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public async Task<IActionResult> UpdateEdge(
        string name,
        string id,
        [FromBody] CreateEdgeRequest request,
        CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        try
        {
            return Ok(await _client.UpdateEdgeAsync(name, id, request, ct));
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.NotFound)
        {
            return NotFound();
        }
    }

    /// <summary>Deletes an edge by ID.</summary>
    [HttpDelete("{id}")]
    [ProducesResponseType(StatusCodes.Status204NoContent)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public async Task<IActionResult> DeleteEdge(string name, string id, CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        try
        {
            await _client.DeleteEdgeAsync(name, id, ct);
            return NoContent();
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
