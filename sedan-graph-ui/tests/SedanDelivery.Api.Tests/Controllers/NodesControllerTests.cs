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
/// Integration tests for <see cref="SedanDelivery.Api.Controllers.NodesController"/>.
/// Uses <see cref="WebApplicationFactory{TProgram}"/> to spin up the real ASP.NET pipeline
/// with <see cref="GqlApiClient"/> replaced by a Moq mock.
/// </summary>
public sealed class NodesControllerTests : IClassFixture<WebApplicationFactory<Program>>
{
    private readonly WebApplicationFactory<Program> _factory;
    private static readonly JsonSerializerOptions _json = new(JsonSerializerDefaults.Web);

    public NodesControllerTests(WebApplicationFactory<Program> factory)
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

    // ── GetNodes ──────────────────────────────────────────────────────────

    [Fact]
    public async Task GetNodes_ValidGraph_ReturnsOk()
    {
        // Arrange
        var expectedNodes = new List<GraphNode>
        {
            new("node-1", ["Person"], new Dictionary<string, JsonElement>()),
            new("node-2", ["Company"], new Dictionary<string, JsonElement>()),
        };

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.GetNodesAsync("test-graph", It.IsAny<CancellationToken>()))
                .ReturnsAsync(expectedNodes);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs/test-graph/nodes");

        // Assert
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        var body = await response.Content.ReadFromJsonAsync<List<GraphNode>>(_json);
        Assert.NotNull(body);
        Assert.Equal(2, body.Count);
        Assert.Equal("node-1", body[0].Id);
    }

    [Fact]
    public async Task GetNodes_InvalidGraphName_ReturnsBadRequest()
    {
        // Arrange – no mock setup needed; validation fires before service call
        var factory = CreateFactory(mock => { });
        var client = factory.CreateClient();

        // Act – graph name contains special characters
        var response = await client.GetAsync("/api/graphs/invalid!name/nodes");

        // Assert
        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    [Fact]
    public async Task GetNodes_GraphNotFound_Returns5xx()
    {
        // Arrange – GetNodes has no catch block, so upstream 404 surfaces as 500
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.GetNodesAsync("missing-graph", It.IsAny<CancellationToken>()))
                .ThrowsAsync(new HttpRequestException(
                    "gql-api returned 404 Not Found",
                    inner: null,
                    statusCode: HttpStatusCode.NotFound));
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs/missing-graph/nodes");

        // Assert – GetNodes does not handle 404 explicitly; it propagates as 5xx
        Assert.True(
            (int)response.StatusCode >= 500,
            $"Expected 5xx but got {(int)response.StatusCode}");
    }

    [Fact]
    public async Task GetNodes_NetworkError_Returns5xx()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.GetNodesAsync("test-graph", It.IsAny<CancellationToken>()))
                .ThrowsAsync(new HttpRequestException("Network error"));
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs/test-graph/nodes");

        // Assert – unhandled network errors propagate as 5xx
        Assert.True(
            (int)response.StatusCode >= 500,
            $"Expected 5xx but got {(int)response.StatusCode}");
    }

    // ── CreateNode ────────────────────────────────────────────────────────

    [Fact]
    public async Task CreateNode_ValidRequest_ReturnsCreated()
    {
        // Arrange
        var request = new CreateNodeRequest(
            Labels: ["Person"],
            Properties: new Dictionary<string, JsonElement>());

        var createdNode = new GraphNode("node-99", ["Person"], new Dictionary<string, JsonElement>());

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.CreateNodeAsync(
                    "test-graph",
                    It.IsAny<CreateNodeRequest>(),
                    It.IsAny<CancellationToken>()))
                .ReturnsAsync(createdNode);

            // GetNode is called by CreatedAtAction route resolution
            mock.Setup(c => c.GetNodeAsync(
                    "test-graph",
                    "node-99",
                    It.IsAny<CancellationToken>()))
                .ReturnsAsync(createdNode);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.PostAsJsonAsync("/api/graphs/test-graph/nodes", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.Created, response.StatusCode);
        Assert.NotNull(response.Headers.Location);

        var body = await response.Content.ReadFromJsonAsync<GraphNode>(_json);
        Assert.NotNull(body);
        Assert.Equal("node-99", body.Id);
    }

    [Fact]
    public async Task CreateNode_InvalidGraphName_ReturnsBadRequest()
    {
        // Arrange
        var factory = CreateFactory(mock => { });
        var client = factory.CreateClient();

        var request = new CreateNodeRequest(
            Labels: ["Person"],
            Properties: new Dictionary<string, JsonElement>());

        // Act – name with spaces is invalid
        var response = await client.PostAsJsonAsync(
            "/api/graphs/invalid name/nodes", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    // ── GetNode ───────────────────────────────────────────────────────────

    [Fact]
    public async Task GetNode_ReturnsOk()
    {
        // Arrange
        var node = new GraphNode("node-1", ["Person"], new Dictionary<string, JsonElement>());

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.GetNodeAsync("test-graph", "node-1", It.IsAny<CancellationToken>()))
                .ReturnsAsync(node);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs/test-graph/nodes/node-1");

        // Assert
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        var body = await response.Content.ReadFromJsonAsync<GraphNode>(_json);
        Assert.NotNull(body);
        Assert.Equal("node-1", body.Id);
    }

    [Fact]
    public async Task GetNode_NotFound_Returns404()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.GetNodeAsync("test-graph", "no-such-node", It.IsAny<CancellationToken>()))
                .ThrowsAsync(new HttpRequestException(
                    "gql-api returned 404 Not Found",
                    inner: null,
                    statusCode: HttpStatusCode.NotFound));
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs/test-graph/nodes/no-such-node");

        // Assert
        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
    }

    // ── UpdateNode ────────────────────────────────────────────────────────

    [Fact]
    public async Task UpdateNode_ReturnsOk()
    {
        // Arrange
        var request = new CreateNodeRequest(
            Labels: ["Person"],
            Properties: new Dictionary<string, JsonElement>());

        var updated = new GraphNode("node-1", ["Person"], new Dictionary<string, JsonElement>());

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.UpdateNodeAsync(
                    "test-graph",
                    "node-1",
                    It.IsAny<CreateNodeRequest>(),
                    It.IsAny<CancellationToken>()))
                .ReturnsAsync(updated);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.PutAsJsonAsync(
            "/api/graphs/test-graph/nodes/node-1", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        var body = await response.Content.ReadFromJsonAsync<GraphNode>(_json);
        Assert.NotNull(body);
        Assert.Equal("node-1", body.Id);
    }

    // ── DeleteNode ────────────────────────────────────────────────────────

    [Fact]
    public async Task DeleteNode_ReturnsNoContent()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.DeleteNodeAsync("test-graph", "node-1", It.IsAny<CancellationToken>()))
                .Returns(Task.CompletedTask);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.DeleteAsync("/api/graphs/test-graph/nodes/node-1");

        // Assert
        Assert.Equal(HttpStatusCode.NoContent, response.StatusCode);
    }

    [Fact]
    public async Task DeleteNode_NotFound_Returns404()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.DeleteNodeAsync("test-graph", "ghost-node", It.IsAny<CancellationToken>()))
                .ThrowsAsync(new HttpRequestException(
                    "gql-api returned 404 Not Found",
                    inner: null,
                    statusCode: HttpStatusCode.NotFound));
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.DeleteAsync("/api/graphs/test-graph/nodes/ghost-node");

        // Assert
        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
    }
}
