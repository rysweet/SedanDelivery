using System.Net;
using System.Net.Http.Json;
using System.Text.Json;
using Microsoft.AspNetCore.Mvc.Testing;
using Microsoft.Extensions.DependencyInjection;
using Moq;
using SedanDelivery.Api.Models;
using SedanDelivery.Api.Services;
using Xunit;

namespace SedanDelivery.Api.Tests.Controllers;

/// <summary>
/// Integration tests for <see cref="SedanDelivery.Api.Controllers.QueryController"/>.
/// </summary>
public sealed class QueryControllerTests : IClassFixture<WebApplicationFactory<Program>>
{
    private readonly WebApplicationFactory<Program> _factory;
    private static readonly JsonSerializerOptions _json = new(JsonSerializerDefaults.Web);

    public QueryControllerTests(WebApplicationFactory<Program> factory)
    {
        _factory = factory;
    }

    private WebApplicationFactory<Program> CreateFactory(Action<Mock<GqlApiClient>> setup)
    {
        var mock = new Mock<GqlApiClient>(MockBehavior.Strict,
            new HttpClient { BaseAddress = new Uri("http://localhost") });

        setup(mock);

        return _factory.WithWebHostBuilder(builder =>
        {
            builder.ConfigureServices(services =>
            {
                var descriptor = services.SingleOrDefault(
                    d => d.ServiceType == typeof(GqlApiClient));
                if (descriptor is not null)
                {
                    services.Remove(descriptor);
                }

                services.AddSingleton(mock.Object);
            });
        });
    }

    // ── ExecuteQuery ──────────────────────────────────────────────────────

    [Fact]
    public async Task ExecuteQuery_ReturnsOk_WithValidCypher()
    {
        // Arrange
        const string graphName = "test-graph";
        var request = new QueryRequest("MATCH (n) RETURN n LIMIT 10");
        var expectedResponse = new QueryResponse(
            Columns: ["n"],
            Rows: [new Dictionary<string, JsonElement>
            {
                ["n"] = JsonDocument.Parse("{\"id\":\"1\"}").RootElement
            }],
            ElapsedMs: 42);

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.ExecuteQueryAsync(
                    graphName,
                    It.Is<QueryRequest>(r => r.Cypher == request.Cypher),
                    It.IsAny<CancellationToken>()))
                .ReturnsAsync(expectedResponse);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.PostAsJsonAsync(
            $"/api/graphs/{graphName}/query", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        var body = await response.Content.ReadFromJsonAsync<QueryResponse>(_json);
        Assert.NotNull(body);
        Assert.Single(body.Columns);
        Assert.Equal("n", body.Columns[0]);
        Assert.Single(body.Rows);
        Assert.Equal(42, body.ElapsedMs);
    }

    [Fact]
    public async Task ExecuteQuery_ReturnsBadRequest_WhenCypherIsEmpty()
    {
        // Arrange – mock is not called because validation rejects early
        var factory = CreateFactory(mock => { });
        var client = factory.CreateClient();
        var request = new QueryRequest("");

        // Act
        var response = await client.PostAsJsonAsync(
            "/api/graphs/test-graph/query", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    [Fact]
    public async Task ExecuteQuery_ReturnsBadRequest_WhenCypherIsWhitespace()
    {
        // Arrange
        var factory = CreateFactory(mock => { });
        var client = factory.CreateClient();
        var request = new QueryRequest("   ");

        // Act
        var response = await client.PostAsJsonAsync(
            "/api/graphs/test-graph/query", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    [Fact]
    public async Task ExecuteQuery_ReturnsBadRequest_WhenGraphNameIsInvalid()
    {
        // Arrange
        var factory = CreateFactory(mock => { });
        var client = factory.CreateClient();
        var request = new QueryRequest("MATCH (n) RETURN n");

        // Act – graph name exceeds 64 characters which violates the pattern
        var longName = new string('a', 65);
        var response = await client.PostAsJsonAsync(
            $"/api/graphs/{longName}/query", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    [Fact]
    public async Task ExecuteQuery_ReturnsNotFound_WhenGraphDoesNotExist()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.ExecuteQueryAsync(
                    "missing-graph",
                    It.IsAny<QueryRequest>(),
                    It.IsAny<CancellationToken>()))
                .ThrowsAsync(new HttpRequestException(
                    "gql-api returned 404 Not Found",
                    inner: null,
                    statusCode: HttpStatusCode.NotFound));
        });

        var client = factory.CreateClient();
        var request = new QueryRequest("MATCH (n) RETURN n");

        // Act
        var response = await client.PostAsJsonAsync(
            "/api/graphs/missing-graph/query", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
    }

    [Fact]
    public async Task ExecuteQuery_ReturnsBadRequest_WhenUpstreamReturnsBadRequest()
    {
        // Arrange – upstream rejects the Cypher syntax
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.ExecuteQueryAsync(
                    "test-graph",
                    It.IsAny<QueryRequest>(),
                    It.IsAny<CancellationToken>()))
                .ThrowsAsync(new HttpRequestException(
                    "gql-api returned 400 Bad Request: syntax error near MATC",
                    inner: null,
                    statusCode: HttpStatusCode.BadRequest));
        });

        var client = factory.CreateClient();
        var request = new QueryRequest("MATC (n) RETURN n");

        // Act
        var response = await client.PostAsJsonAsync(
            "/api/graphs/test-graph/query", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);

        var problem = await response.Content.ReadFromJsonAsync<JsonElement>(_json);
        Assert.True(problem.TryGetProperty("title", out _),
            "Error response should be a ProblemDetails object");
    }

    // ── ExecuteMutation ───────────────────────────────────────────────────

    [Fact]
    public async Task ExecuteMutation_ReturnsOk_WithValidCypher()
    {
        // Arrange
        const string graphName = "test-graph";
        var request = new MutationRequest("CREATE (n:Person {name: 'Alice'})");
        var expectedResponse = new MutationResponse(
            NodesCreated: 1, NodesDeleted: 0,
            EdgesCreated: 0, EdgesDeleted: 0,
            PropertiesSet: 1);

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.ExecuteMutationAsync(
                    graphName,
                    It.Is<MutationRequest>(r => r.Cypher == request.Cypher),
                    It.IsAny<CancellationToken>()))
                .ReturnsAsync(expectedResponse);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.PostAsJsonAsync(
            $"/api/graphs/{graphName}/mutate", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        var body = await response.Content.ReadFromJsonAsync<MutationResponse>(_json);
        Assert.NotNull(body);
        Assert.Equal(1, body.NodesCreated);
        Assert.Equal(1, body.PropertiesSet);
    }

    [Fact]
    public async Task ExecuteMutation_ReturnsBadRequest_WhenCypherIsEmpty()
    {
        // Arrange
        var factory = CreateFactory(mock => { });
        var client = factory.CreateClient();
        var request = new MutationRequest("");

        // Act
        var response = await client.PostAsJsonAsync(
            "/api/graphs/test-graph/mutate", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }
}
