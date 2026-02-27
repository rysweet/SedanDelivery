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
/// Integration tests for <see cref="SedanDelivery.Api.Controllers.GraphsController"/>.
/// Uses <see cref="WebApplicationFactory{TProgram}"/> to spin up the real ASP.NET pipeline
/// with the <see cref="GqlApiClient"/> replaced by a Moq mock.
/// </summary>
public sealed class GraphsControllerTests : IClassFixture<WebApplicationFactory<Program>>
{
    private readonly WebApplicationFactory<Program> _factory;
    private static readonly JsonSerializerOptions _json = new(JsonSerializerDefaults.Web);

    public GraphsControllerTests(WebApplicationFactory<Program> factory)
    {
        _factory = factory;
    }

    private WebApplicationFactory<Program> CreateFactory(Action<Mock<GqlApiClient>> setup)
    {
        var mock = new Mock<GqlApiClient>(MockBehavior.Strict,
            // GqlApiClient has a single constructor parameter (HttpClient).
            // We pass a dummy client; the mock will intercept all calls.
            new HttpClient { BaseAddress = new Uri("http://localhost") });

        setup(mock);

        return _factory.WithWebHostBuilder(builder =>
        {
            builder.ConfigureServices(services =>
            {
                // Remove the real GqlApiClient registration and replace with the mock.
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

    // ── GetGraphs ─────────────────────────────────────────────────────────

    [Fact]
    public async Task GetGraphs_ReturnsOk_WithListOfGraphs()
    {
        // Arrange
        var expectedGraphs = new List<GraphInfo>
        {
            new("test-graph", 10, 5),
            new("another-graph", 3, 1)
        };

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.GetGraphsAsync(It.IsAny<CancellationToken>()))
                .ReturnsAsync(expectedGraphs);
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs");

        // Assert
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        var body = await response.Content.ReadFromJsonAsync<List<GraphInfo>>(_json);
        Assert.NotNull(body);
        Assert.Equal(2, body.Count);
        Assert.Equal("test-graph", body[0].Name);
        Assert.Equal(10, body[0].NodeCount);
    }

    [Fact]
    public async Task GetGraphs_ReturnsOk_WithEmptyList()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.GetGraphsAsync(It.IsAny<CancellationToken>()))
                .ReturnsAsync(new List<GraphInfo>());
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs");

        // Assert
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        var body = await response.Content.ReadFromJsonAsync<List<GraphInfo>>(_json);
        Assert.NotNull(body);
        Assert.Empty(body);
    }

    // ── CreateGraph ───────────────────────────────────────────────────────

    [Fact]
    public async Task CreateGraph_ReturnsCreated_WhenNameIsValid()
    {
        // Arrange
        var request = new CreateGraphRequest("my-new-graph");
        var created = new GraphInfo("my-new-graph", 0, 0);

        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.CreateGraphAsync(
                    It.Is<CreateGraphRequest>(r => r.Name == "my-new-graph"),
                    It.IsAny<CancellationToken>()))
                .ReturnsAsync(created);

            // GetGraphStats is called by CreatedAtAction route resolution
            mock.Setup(c => c.GetGraphStatsAsync(
                    It.IsAny<string>(),
                    It.IsAny<CancellationToken>()))
                .ReturnsAsync(new GraphStats("my-new-graph", 0, 0,
                    new Dictionary<string, long>(), new Dictionary<string, long>()));
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.PostAsJsonAsync("/api/graphs", request, _json);

        // Assert
        Assert.Equal(HttpStatusCode.Created, response.StatusCode);
        Assert.NotNull(response.Headers.Location);

        var body = await response.Content.ReadFromJsonAsync<GraphInfo>(_json);
        Assert.NotNull(body);
        Assert.Equal("my-new-graph", body.Name);
    }

    [Theory]
    [InlineData("")]
    [InlineData("this-name-is-way-too-long-and-exceeds-the-maximum-allowed-length-of-sixty-four-characters")]
    [InlineData("invalid name with spaces")]
    [InlineData("has!special@chars")]
    public async Task CreateGraph_ReturnsBadRequest_WhenNameIsInvalid(string invalidName)
    {
        // Arrange – mock is set up but should NOT be called for invalid names
        var factory = CreateFactory(mock => { /* no setup needed */ });
        var client = factory.CreateClient();

        // Act
        var response = await client.PostAsJsonAsync(
            "/api/graphs", new CreateGraphRequest(invalidName), _json);

        // Assert
        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    // ── ResponseHeaders ───────────────────────────────────────────────────

    [Fact]
    public async Task AllResponses_ContainXRequestIdHeader()
    {
        // Arrange
        var factory = CreateFactory(mock =>
        {
            mock.Setup(c => c.GetGraphsAsync(It.IsAny<CancellationToken>()))
                .ReturnsAsync(new List<GraphInfo>());
        });

        var client = factory.CreateClient();

        // Act
        var response = await client.GetAsync("/api/graphs");

        // Assert
        Assert.True(response.Headers.Contains("X-Request-Id"),
            "Every response must include X-Request-Id header");
    }
}
