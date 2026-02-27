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
/// Integration tests for <see cref="SedanDelivery.Api.Controllers.EdgesController"/>.
/// Uses <see cref="WebApplicationFactory{TProgram}"/> to spin up the real ASP.NET pipeline
/// with <see cref="GqlApiClient"/> replaced by a Moq mock.
/// </summary>
public sealed class EdgesControllerTests : IClassFixture<WebApplicationFactory<Program>>
{
    private readonly WebApplicationFactory<Program> _factory;
    private static readonly JsonSerializerOptions _json = new(JsonSerializerDefaults.Web);

    public EdgesControllerTests(WebApplicationFactory<Program> factory)
    {
        _factory = factory;
    }

    private WebApplicationFactory<Program> CreateFactory(Action<Mock<IGqlApiClient>> setup)
    {
        var mock = new Mock<IGqlApiClient>(MockBehavior.Strict);

        setup(mock);

        return _factory.WithWebHostBuilder(builder =>
        {
            builder.ConfigureServices(services =>
            {
                var descriptor = services.SingleOrDefault(
                    d => d.ServiceType == typeof(IGqlApiClient));
                if (descriptor is not null)
                {
                    services.Remove(descriptor);
                }

                services.AddSingleton(mock.Object);
            });
        });
    }

    // ── GetEdges ──────────────────────────────────────────────────────────

    [Fact]
    public async Task GetEdges_ValidGraph_ReturnsOk()
    {
        // Arrange
        var expectedEdges = new List<GraphEdge>
        {
            new("edge-1", "KNOWS", "node-1", "node-2", new Dictionary<string, JsonElement>()),
            new("edge-2", "WORKS_AT", "node-2", "node-3", new Dictionary<string, JsonElement>()),
        };

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.GetEdgesAsync("test-graph", It.IsAny<CancellationToken>()))
                .ReturnsAsync(expectedEdges);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs/test-graph/edges");

        // Assert
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        var body = await response.Content.ReadFromJsonAsync<List<GraphEdge>>(_json);
        Assert.NotNull(body);
        Assert.Equal(2, body.Count);
        Assert.Equal("edge-1", body[0].Id);
        Assert.Equal("KNOWS", body[0].Type);
    }

    [Fact]
    public async Task GetEdges_InvalidGraphName_ReturnsBadRequest()
    {
        // Arrange – no mock setup; validation rejects before service call
        var factory = CreateFactory(mock => { });
        var client = factory.CreateClient();

        // Act – graph name contains special characters
        var response = await client.GetAsync("/api/graphs/bad!name/edges");

        // Assert
        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    [Fact]
    public async Task GetEdges_GraphNotFound_Returns5xx()
    {
        // Arrange – GetEdges has no catch block, so upstream 404 surfaces as 500
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.GetEdgesAsync("missing-graph", It.IsAny<CancellationToken>()))
                .ThrowsAsync(new HttpRequestException(
                    "gql-api returned 404 Not Found",
                    inner: null,
                    statusCode: HttpStatusCode.NotFound));
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs/missing-graph/edges");

        // Assert – GetEdges does not handle 404 explicitly; it propagates as 5xx
        Assert.True(
            (int)response.StatusCode >= 500,
            $"Expected 5xx but got {(int)response.StatusCode}");
    }

    [Fact]
    public async Task GetEdges_NetworkError_Returns5xx()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.GetEdgesAsync("test-graph", It.IsAny<CancellationToken>()))
                .ThrowsAsync(new HttpRequestException("Network error"));
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs/test-graph/edges");

        // Assert – unhandled network errors propagate as 5xx
        Assert.True(
            (int)response.StatusCode >= 500,
            $"Expected 5xx but got {(int)response.StatusCode}");
    }

    // ── CreateEdge ────────────────────────────────────────────────────────

    [Fact]
    public async Task CreateEdge_ValidRequest_ReturnsCreated()
    {
        // Arrange
        var request = new CreateEdgeRequest(
            Source: "node-1",
            Target: "node-2",
            Type: "KNOWS",
            Properties: new Dictionary<string, JsonElement>());

        var createdEdge = new GraphEdge(
            "edge-99", "KNOWS", "node-1", "node-2", new Dictionary<string, JsonElement>());

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.CreateEdgeAsync(
                    "test-graph",
                    It.IsAny<CreateEdgeRequest>(),
                    It.IsAny<CancellationToken>()))
                .ReturnsAsync(createdEdge);

            // GetEdge is called by CreatedAtAction route resolution
            mock.Setup(c => c.GetEdgeAsync(
                    "test-graph",
                    "edge-99",
                    It.IsAny<CancellationToken>()))
                .ReturnsAsync(createdEdge);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.PostAsJsonAsync(
            "/api/graphs/test-graph/edges", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.Created, response.StatusCode);
        Assert.NotNull(response.Headers.Location);

        var body = await response.Content.ReadFromJsonAsync<GraphEdge>(_json);
        Assert.NotNull(body);
        Assert.Equal("edge-99", body.Id);
        Assert.Equal("KNOWS", body.Type);
    }

    [Fact]
    public async Task CreateEdge_InvalidGraphName_ReturnsBadRequest()
    {
        // Arrange
        var factory = CreateFactory(mock => { });
        var client = factory.CreateClient();

        var request = new CreateEdgeRequest(
            Source: "node-1",
            Target: "node-2",
            Type: "KNOWS",
            Properties: new Dictionary<string, JsonElement>());

        // Act – name with spaces is invalid
        var response = await client.PostAsJsonAsync(
            "/api/graphs/invalid name/edges", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    // ── GetEdge ───────────────────────────────────────────────────────────

    [Fact]
    public async Task GetEdge_ReturnsOk()
    {
        // Arrange
        var edge = new GraphEdge(
            "edge-1", "KNOWS", "node-1", "node-2", new Dictionary<string, JsonElement>());

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.GetEdgeAsync("test-graph", "edge-1", It.IsAny<CancellationToken>()))
                .ReturnsAsync(edge);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs/test-graph/edges/edge-1");

        // Assert
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        var body = await response.Content.ReadFromJsonAsync<GraphEdge>(_json);
        Assert.NotNull(body);
        Assert.Equal("edge-1", body.Id);
    }

    [Fact]
    public async Task GetEdge_NotFound_Returns404()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.GetEdgeAsync("test-graph", "no-such-edge", It.IsAny<CancellationToken>()))
                .ThrowsAsync(new HttpRequestException(
                    "gql-api returned 404 Not Found",
                    inner: null,
                    statusCode: HttpStatusCode.NotFound));
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs/test-graph/edges/no-such-edge");

        // Assert
        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
    }

    // ── UpdateEdge ────────────────────────────────────────────────────────

    [Fact]
    public async Task UpdateEdge_ReturnsOk()
    {
        // Arrange
        var request = new CreateEdgeRequest(
            Source: "node-1",
            Target: "node-2",
            Type: "KNOWS",
            Properties: new Dictionary<string, JsonElement>());

        var updated = new GraphEdge(
            "edge-1", "KNOWS", "node-1", "node-2", new Dictionary<string, JsonElement>());

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.UpdateEdgeAsync(
                    "test-graph",
                    "edge-1",
                    It.IsAny<CreateEdgeRequest>(),
                    It.IsAny<CancellationToken>()))
                .ReturnsAsync(updated);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.PutAsJsonAsync(
            "/api/graphs/test-graph/edges/edge-1", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        var body = await response.Content.ReadFromJsonAsync<GraphEdge>(_json);
        Assert.NotNull(body);
        Assert.Equal("edge-1", body.Id);
    }

    // ── DeleteEdge ────────────────────────────────────────────────────────

    [Fact]
    public async Task DeleteEdge_ReturnsNoContent()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.DeleteEdgeAsync("test-graph", "edge-1", It.IsAny<CancellationToken>()))
                .Returns(Task.CompletedTask);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.DeleteAsync("/api/graphs/test-graph/edges/edge-1");

        // Assert
        Assert.Equal(HttpStatusCode.NoContent, response.StatusCode);
    }

    [Fact]
    public async Task DeleteEdge_NotFound_Returns404()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.DeleteEdgeAsync("test-graph", "ghost-edge", It.IsAny<CancellationToken>()))
                .ThrowsAsync(new HttpRequestException(
                    "gql-api returned 404 Not Found",
                    inner: null,
                    statusCode: HttpStatusCode.NotFound));
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.DeleteAsync("/api/graphs/test-graph/edges/ghost-edge");

        // Assert
        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
    }
}
