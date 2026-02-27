using Aspire.Hosting;

var builder = DistributedApplication.CreateBuilder(args);

// gql-api Rust service listens on port 3001.
// In development it is expected to already be running (`cargo run -p gql-api`).
// We model it as an executable resource so Aspire tracks and surfaces its
// endpoint in the dashboard.
var worktreeRoot = Path.GetFullPath(
    Path.Combine(builder.AppHostDirectory, "..", "..", "..", ".."));

var gqlApiPath = Path.Combine(worktreeRoot, "run.sh");

// Register the Rust gql-api as an executable resource on port 3001.
// ExecutableResource does not implement IResourceWithConnectionString so we
// wire the URL via WithEnvironment on the downstream project.
var gqlApi = builder
    .AddExecutable("gql-api", gqlApiPath, workingDirectory: worktreeRoot)
    .WithHttpEndpoint(port: 3001, name: "http");

// Register the BFF API project and inject GQL_API_URL so GqlApiClient
// knows where to find the Rust service.
var api = builder
    .AddProject<Projects.SedanDelivery_Api>("sedan-api")
    .WithEnvironment("GQL_API_URL", "http://localhost:3001")
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
