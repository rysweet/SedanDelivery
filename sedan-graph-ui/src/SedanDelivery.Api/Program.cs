using System.Threading.RateLimiting;
using Microsoft.AspNetCore.RateLimiting;
using Scalar.AspNetCore;
using SedanDelivery.Api.Services;

var builder = WebApplication.CreateBuilder(args);

// ── Aspire service defaults ────────────────────────────────────────────────
builder.AddServiceDefaults();

// ── CORS ───────────────────────────────────────────────────────────────────
// Allow the React Vite dev server and any origins listed in CORS_ORIGINS.
var corsOrigins = (builder.Configuration["CORS_ORIGINS"] ?? "")
    .Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);

builder.Services.AddCors(options =>
{
    options.AddDefaultPolicy(policy =>
    {
        var origins = new List<string> { "http://localhost:5173", "http://localhost:3000" };
        origins.AddRange(corsOrigins);

        policy
            .WithOrigins(origins.ToArray())
            .AllowAnyHeader()
            .AllowAnyMethod()
            .WithExposedHeaders("X-Request-Id");
    });
});

// ── Rate limiting ──────────────────────────────────────────────────────────
// "query" policy: 100 requests per 60 seconds per IP (sliding window).
builder.Services.AddRateLimiter(options =>
{
    options.AddSlidingWindowLimiter("query", limiterOptions =>
    {
        limiterOptions.PermitLimit = 100;
        limiterOptions.Window = TimeSpan.FromSeconds(60);
        limiterOptions.SegmentsPerWindow = 6;
        limiterOptions.QueueProcessingOrder = QueueProcessingOrder.OldestFirst;
        limiterOptions.QueueLimit = 10;
    });

    options.RejectionStatusCode = StatusCodes.Status429TooManyRequests;
});

// ── Typed HttpClient: GqlApiClient ─────────────────────────────────────────
// gql-api Rust service listens on port 3001 by default.
// Aspire injects GQL_API_URL when running under AppHost.
var gqlApiUrl = builder.Configuration["GQL_API_URL"]
    ?? throw new InvalidOperationException(
        "GQL_API_URL is required. Set it to the gql-api base URL (e.g. http://localhost:3001). " +
        "When running under .NET Aspire this is injected automatically.");

builder.Services.AddHttpClient<GqlApiClient>(client =>
{
    client.BaseAddress = new Uri(gqlApiUrl);
    client.DefaultRequestHeaders.Add("Accept", "application/json");
    client.Timeout = TimeSpan.FromSeconds(30);
});

// ── Problem Details ────────────────────────────────────────────────────────
// Registers the ProblemDetails services so Problem() helper methods serialize
// correctly in all environments including test hosts.
builder.Services.AddProblemDetails();

// ── MVC / Controllers ──────────────────────────────────────────────────────
builder.Services.AddControllers()
    .AddJsonOptions(options =>
    {
        options.JsonSerializerOptions.PropertyNamingPolicy =
            System.Text.Json.JsonNamingPolicy.CamelCase;
        options.JsonSerializerOptions.DefaultIgnoreCondition =
            System.Text.Json.Serialization.JsonIgnoreCondition.WhenWritingNull;
    });

// ── OpenAPI (built-in .NET 10 provider) ───────────────────────────────────
builder.Services.AddOpenApi(options =>
{
    options.AddDocumentTransformer((document, context, _) =>
    {
        document.Info.Title = "SedanDelivery Graph API";
        document.Info.Version = "v1";
        document.Info.Description =
            "BFF that proxies graph operations to the Rust gql-api service.";
        return Task.CompletedTask;
    });
});

// ── Build ──────────────────────────────────────────────────────────────────
var app = builder.Build();

// ── Middleware pipeline ────────────────────────────────────────────────────

// Attach a correlation ID to every response.
app.Use(async (context, next) =>
{
    var requestId = context.Request.Headers["X-Request-Id"].FirstOrDefault()
        ?? Guid.NewGuid().ToString("N");
    context.Response.Headers["X-Request-Id"] = requestId;
    await next(context);
});

if (app.Environment.IsDevelopment())
{
    // Serve the OpenAPI JSON document at /openapi/v1.json
    app.MapOpenApi();
    // Scalar API reference UI at /scalar/v1
    app.MapScalarApiReference();
}

app.UseRateLimiter();
app.UseCors();
app.UseAuthorization();
app.MapControllers();
app.MapDefaultEndpoints(); // /health and /alive from ServiceDefaults

app.Run();

// Expose Program for WebApplicationFactory in tests.
public partial class Program { }
