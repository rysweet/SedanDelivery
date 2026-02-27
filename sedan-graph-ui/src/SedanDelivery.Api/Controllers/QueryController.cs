using Microsoft.AspNetCore.Mvc;
using Microsoft.AspNetCore.RateLimiting;
using SedanDelivery.Api.Models;
using SedanDelivery.Api.Services;
using SedanDelivery.Api.Validation;

namespace SedanDelivery.Api.Controllers;

/// <summary>
/// Executes read (query) and write (mutate) Cypher statements against a named graph.
/// Both endpoints are rate-limited to 100 requests per minute per IP.
/// </summary>
[ApiController]
[Route("api/graphs/{name}")]
[Produces("application/json")]
public sealed class QueryController : ControllerBase
{
    private readonly GqlApiClient _client;
    private readonly ILogger<QueryController> _logger;

    public QueryController(GqlApiClient client, ILogger<QueryController> logger)
    {
        _client = client;
        _logger = logger;
    }

    /// <summary>
    /// Executes a read-only Cypher query and returns column names, rows, and elapsed time.
    /// </summary>
    [HttpPost("query")]
    [EnableRateLimiting("query")]
    [ProducesResponseType(typeof(QueryResponse), StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    [ProducesResponseType(StatusCodes.Status429TooManyRequests)]
    public async Task<IActionResult> ExecuteQuery(
        string name,
        [FromBody] QueryRequest request,
        CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name))
        {
            return InvalidName();
        }

        if (string.IsNullOrWhiteSpace(request.Cypher))
        {
            return Problem(
                title: "Invalid request",
                detail: "cypher must not be empty",
                statusCode: StatusCodes.Status400BadRequest);
        }

        try
        {
            var result = await _client.ExecuteQueryAsync(name, request, ct);
            return Ok(result);
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.NotFound)
        {
            return NotFound();
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.BadRequest)
        {
            return Problem(
                title: "Query error",
                detail: ex.Message,
                statusCode: StatusCodes.Status400BadRequest);
        }
    }

    /// <summary>
    /// Executes a write Cypher statement and returns mutation statistics.
    /// </summary>
    [HttpPost("mutate")]
    [EnableRateLimiting("query")]
    [ProducesResponseType(typeof(MutationResponse), StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    [ProducesResponseType(StatusCodes.Status429TooManyRequests)]
    public async Task<IActionResult> ExecuteMutation(
        string name,
        [FromBody] MutationRequest request,
        CancellationToken ct)
    {
        if (!GraphNameValidator.IsValid(name))
        {
            return InvalidName();
        }

        if (string.IsNullOrWhiteSpace(request.Cypher))
        {
            return Problem(
                title: "Invalid request",
                detail: "cypher must not be empty",
                statusCode: StatusCodes.Status400BadRequest);
        }

        try
        {
            var result = await _client.ExecuteMutationAsync(name, request, ct);
            return Ok(result);
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.NotFound)
        {
            return NotFound();
        }
        catch (HttpRequestException ex) when (ex.StatusCode == System.Net.HttpStatusCode.BadRequest)
        {
            return Problem(
                title: "Mutation error",
                detail: ex.Message,
                statusCode: StatusCodes.Status400BadRequest);
        }
    }

    private IActionResult InvalidName() =>
        Problem(
            title: "Invalid graph name",
            detail: "Graph name must match ^[a-zA-Z0-9_.\\-]{1,64}$",
            statusCode: StatusCodes.Status400BadRequest);
}
