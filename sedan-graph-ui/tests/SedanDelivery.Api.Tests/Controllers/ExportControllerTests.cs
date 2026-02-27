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
/// Integration tests for <see cref="SedanDelivery.Api.Controllers.ExportController"/>.
/// Uses <see cref="WebApplicationFactory{TProgram}"/> to spin up the real ASP.NET pipeline
/// with <see cref="GqlApiClient"/> replaced by a Moq mock.
/// </summary>
public sealed class ExportControllerTests : IClassFixture<WebApplicationFactory<Program>>
{
    private readonly WebApplicationFactory<Program> _factory;
    private static readonly JsonSerializerOptions _json = new(JsonSerializerDefaults.Web);

    public ExportControllerTests(WebApplicationFactory<Program> factory)
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

    // ── ExportGraph ───────────────────────────────────────────────────────

    [Fact]
    public async Task ExportGraph_ReturnsOk()
    {
        // Arrange – mock returns full graph export data
        var exportData = new ExportGraph(
            Nodes:
            [
                new GraphNode("node-1", ["Person"], new Dictionary<string, JsonElement>()),
                new GraphNode("node-2", ["Company"], new Dictionary<string, JsonElement>()),
            ],
            Edges:
            [
                new GraphEdge("edge-1", "WORKS_AT", "node-1", "node-2", new Dictionary<string, JsonElement>()),
            ]);

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.ExportGraphAsync("test-graph", It.IsAny<CancellationToken>()))
                .ReturnsAsync(exportData);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs/test-graph/export");

        // Assert
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        var body = await response.Content.ReadFromJsonAsync<ExportGraph>(_json);
        Assert.NotNull(body);
        Assert.Equal(2, body.Nodes.Count);
        Assert.Single(body.Edges);
        Assert.Equal("node-1", body.Nodes[0].Id);
    }

    [Fact]
    public async Task ExportGraph_NotFound_Returns404()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.ExportGraphAsync("missing-graph", It.IsAny<CancellationToken>()))
                .ThrowsAsync(new HttpRequestException(
                    "gql-api returned 404 Not Found",
                    inner: null,
                    statusCode: HttpStatusCode.NotFound));
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs/missing-graph/export");

        // Assert
        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
    }

    [Fact]
    public async Task ExportGraph_NetworkError_Returns5xx()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.ExportGraphAsync("test-graph", It.IsAny<CancellationToken>()))
                .ThrowsAsync(new HttpRequestException("Network error"));
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs/test-graph/export");

        // Assert – unhandled network errors propagate as 5xx
        Assert.True(
            (int)response.StatusCode >= 500,
            $"Expected 5xx but got {(int)response.StatusCode}");
    }

    // ── ImportGraph ───────────────────────────────────────────────────────

    [Fact]
    public async Task ImportGraph_ValidPayload_ReturnsOk()
    {
        // Arrange
        var payload = new ImportGraph(
            Nodes:
            [
                new GraphNode("node-1", ["Person"], new Dictionary<string, JsonElement>()),
            ],
            Edges: []);

        var importResult = new ImportResult(NodesImported: 1, EdgesImported: 0);

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.ImportGraphAsync(
                    "test-graph",
                    It.IsAny<ImportGraph>(),
                    It.IsAny<CancellationToken>()))
                .ReturnsAsync(importResult);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.PostAsJsonAsync(
            "/api/graphs/test-graph/import", payload, _json);

        // Assert
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        var body = await response.Content.ReadFromJsonAsync<ImportResult>(_json);
        Assert.NotNull(body);
        Assert.Equal(1, body.NodesImported);
        Assert.Equal(0, body.EdgesImported);
    }

    [Fact]
    public async Task ImportGraph_InvalidGraphName_ReturnsBadRequest()
    {
        // Arrange – no mock setup; validation rejects before service call
        var factory = CreateFactory(mock => { });
        var client = factory.CreateClient();

        var payload = new ImportGraph(Nodes: [], Edges: []);

        // Act – name with special chars is invalid
        var response = await client.PostAsJsonAsync(
            "/api/graphs/bad@name/import", payload, _json);

        // Assert
        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    [Fact]
    public async Task ImportGraph_NotFound_Returns404()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.ImportGraphAsync(
                    "missing-graph",
                    It.IsAny<ImportGraph>(),
                    It.IsAny<CancellationToken>()))
                .ThrowsAsync(new HttpRequestException(
                    "gql-api returned 404 Not Found",
                    inner: null,
                    statusCode: HttpStatusCode.NotFound));
        });

        var client = factory.CreateClient();
        var payload = new ImportGraph(Nodes: [], Edges: []);

        // Act
        var response = await client.PostAsJsonAsync(
            "/api/graphs/missing-graph/import", payload, _json);

        // Assert
        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
    }
}
