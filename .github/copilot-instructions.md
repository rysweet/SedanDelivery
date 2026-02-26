# CyberGym — Copilot Instructions

## Project Overview
CyberGym is a .NET 10 + Aspire 13.1 distributed application for cybersecurity training gameboard management. It consists of web APIs (GameboardTenant, Catalog), CLI tools, and an Aspire AppHost orchestrator.

## Code Quality Standards
This project follows the dotnet/aspire quality bar (David Fowler's team). All code — human and AI-generated — MUST meet these standards.

### Build Configuration
- **TreatWarningsAsErrors=true** — zero warnings allowed
- **EnforceCodeStyleInBuild=true** — style rules enforced at build time
- **AnalysisLevel=latest-Recommended** — latest .NET 10 Roslyn analyzers enabled
- **Nullable=enable** everywhere — no nullable reference type warnings suppressed

### Naming Conventions (MANDATORY)
- Private fields: `_camelCase` (underscore prefix)
- Static private fields: `s_camelCase` (s_ prefix)
- Constants: `PascalCase`
- All other members: PascalCase
- Parameters and locals: camelCase

### Code Style Rules
- **File-scoped namespaces** required (`namespace Foo;` not `namespace Foo { }`)
- **var** preferred everywhere — let the type be inferred
- **Braces required** even for single-line if/else/for/foreach
- **System usings sorted first** (`dotnet_sort_system_directives_first = true`)
- **No `this.` qualifier** — do not prefix with `this.`
- **Predefined types** — use `int` not `Int32`, `string` not `String`

### Performance Patterns (.NET 10)
Stephen Toub's .NET 10 performance work (see his annual "Performance Improvements in .NET" blog series) demonstrates that the JIT's deabstraction (escape analysis, devirtualization) is so advanced that **writing clean idiomatic C# IS the performance optimization**. AI-generated code should follow patterns, not try to be clever.

- Use `ArgumentNullException.ThrowIfNull()` instead of manual null checks (CA1510)
- Use `ArgumentException.ThrowIfNullOrEmpty()` (CA1511)
- Use `ArgumentOutOfRangeException.ThrowIfNegative()` (CA1512)
- Use `Array.Empty<T>()` instead of `new T[0]` (CA1825)
- Use `string.Contains(char)` instead of `string.Contains(string)` for single chars (CA1847)
- Prefer `AsSpan()` over `Substring()` (CA1846)
- Forward `CancellationToken` to all async methods that accept one (CA2016)
- `ConfigureAwait(false)` is only needed in library code (CA2007) — CyberGym silences CA2007 because all projects are ASP.NET Core apps with no SynchronizationContext
- Seal internal types for better JIT devirtualization (CA1852)
- Prefer `TryGetValue` over `ContainsKey` + indexer (CA1854)

### Async/Await
- All I/O-bound operations MUST be async
- Use `ValueTask` for hot paths that often complete synchronously
- Never use `.Result` or `.Wait()` — always `await`
- Always forward `CancellationToken` parameters

### Error Handling
- Never swallow exceptions — log at minimum
- Use `ArgumentNullException.ThrowIfNull()` for null guards
- Use problem details (RFC 9457) for API error responses
- Do NOT use `throw ex;` — use `throw;` to preserve stack traces (CA2200)

### Testing
- xUnit for unit tests, FluentAssertions for assertions
- Test projects have relaxed analyzer rules (suggestions, not warnings)
- Use Testcontainers for integration tests requiring real databases
- Moq for mocking

### Entity Framework
- Always use async EF Core methods (`ToListAsync`, `FirstOrDefaultAsync`, etc.)
- Use `IQueryable` for composable queries, `IEnumerable` for materialized results
- Configure entities with `IEntityTypeConfiguration<T>`, not inline in `OnModelCreating`

### Aspire Patterns
- Use `AddProject<T>()` for service registration in AppHost
- Configure service defaults (health checks, telemetry, resilience) via `AddServiceDefaults()`
- Use Aspire component packages for database and messaging integration

### Security
- JWT Bearer authentication via `Microsoft.AspNetCore.Authentication.JwtBearer`
- Azure Identity (`DefaultAzureCredential`) for Azure service authentication
- Never hardcode secrets — use User Secrets locally, Key Vault in production
- Validate all inputs at API boundaries

### What NOT to Do
- Do NOT suppress warnings with `#pragma warning disable` without a justifying comment
- Do NOT use `NoWarn` in .csproj to hide legitimate issues
- Do NOT write "clever" micro-optimized code — write clean idiomatic C# and let the JIT optimize
- Do NOT use `dynamic` type
- Do NOT use `object` when a generic type parameter is appropriate
- Do NOT create empty catch blocks
- Do NOT ignore the return value of async methods
