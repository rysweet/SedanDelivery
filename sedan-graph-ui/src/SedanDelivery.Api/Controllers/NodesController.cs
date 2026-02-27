using Microsoft.AspNetCore.Mvc;
using SedanDelivery.Api.Models;
using SedanDelivery.Api.Services;
using SedanDelivery.Api.Validation;

namespace SedanDelivery.Api.Controllers;

/// <summary>
/// CRUD operations for nodes within a named graph.
/// </summary>
[ApiController]
[Route("api/graphs/{name}/nodes")]
[Produces("application/json")]
public sealed class NodesController : ControllerBase
{
    private readonly GqlApiClient _client;

    public NodesController(GqlApiClient client)
    {
        _client = client;
    }

    /// <summary>Returns all nodes in the graph.</summary>
    [HttpGet]
    [ProducesResponseType(typeof(List<GraphNode>), StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    public async Task<IActionResult> GetNodes(string name, CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        var nodes = await _client.GetNodesAsync(name, ct);
        return Ok(nodes);
    }

    /// <summary>Creates a new node in the graph.</summary>
    [HttpPost]
    [ProducesResponseType(typeof(GraphNode), StatusCodes.Status201Created)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    public async Task<IActionResult> CreateNode(
        string name,
        [FromBody] CreateNodeRequest request,
        CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        var node = await _client.CreateNodeAsync(name, request, ct);
        return CreatedAtAction(nameof(GetNode), new { name, id = node.Id }, node);
    }

    /// <summary>Returns a single node by ID.</summary>
    [HttpGet("{id}")]
    [ProducesResponseType(typeof(GraphNode), StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public async Task<IActionResult> GetNode(string name, string id, CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        try
        {
            return Ok(await _client.GetNodeAsync(name, id, ct));
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.NotFound)
        {
            return NotFound();
        }
    }

    /// <summary>Replaces a node's labels and properties.</summary>
    [HttpPut("{id}")]
    [ProducesResponseType(typeof(GraphNode), StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public async Task<IActionResult> UpdateNode(
        string name,
        string id,
        [FromBody] CreateNodeRequest request,
        CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        try
        {
            return Ok(await _client.UpdateNodeAsync(name, id, request, ct));
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.NotFound)
        {
            return NotFound();
        }
    }

    /// <summary>Deletes a node by ID.</summary>
    [HttpDelete("{id}")]
    [ProducesResponseType(StatusCodes.Status204NoContent)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public async Task<IActionResult> DeleteNode(string name, string id, CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name)) return InvalidName();
        try
        {
            await _client.DeleteNodeAsync(name, id, ct);
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
