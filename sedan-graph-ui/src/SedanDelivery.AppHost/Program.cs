using Aspire.Hosting;

var builder = DistributedApplication.CreateBuilder(args);

// sd2-api Rust service listens on port 3001.
// In development it is expected to already be running (`cargo run -p sd2-api`).
// We model it as an executable resource so Aspire tracks and surfaces its
// endpoint in the dashboard.
var worktreeRoot = Path.GetFullPath(
    Path.Combine(builder.AppHostDirectory, "..", "..", "..", ".."));

var gqlApiPath = Path.Combine(worktreeRoot, "run.sh");

// Register the Rust sd2-api as an executable resource on port 3001.
// ExecutableResource does not implement IResourceWithConnectionString so we
// wire the URL via WithEnvironment using the Aspire endpoint reference.
var gqlApi = builder
    .AddExecutable("sd2-api", gqlApiPath, workingDirectory: worktreeRoot)
    .WithHttpEndpoint(port: 3001, name: "http");

// Register the BFF API project and inject GQL_API_URL so GqlApiClient
// knows where to find the Rust service. Using a callback-based WithEnvironment
// captures the Aspire-resolved endpoint URL rather than hardcoding localhost.
var api = builder
    .AddProject<Projects.SedanDelivery_Api>("sedan-api")
    .WithEnvironment(ctx =>
    {
        // Resolve the sd2-api endpoint URL through Aspire service discovery.
        // In dev this produces http://localhost:3001; in cloud it uses the
        // Aspire-assigned address for the container/process.
        var gqlEndpoint = gqlApi.GetEndpoint("http");
        ctx.EnvironmentVariables["GQL_API_URL"] = gqlEndpoint;
    })
    .WaitFor(gqlApi);

// In local dev the React SPA is served by the Vite dev server on port 5173.
// Excluded from the Aspire manifest — it is not deployed by Aspire.
builder
    .AddNpmApp(
        "sedan-spa",
        Path.Combine(builder.AppHostDirectory, "..", "..", "sedan-web"),
        "dev")
    .WithReference(api)
    .WithHttpEndpoint(port: 5173, name: "http", env: "PORT")
    .ExcludeFromManifest();

builder.Build().Run();
