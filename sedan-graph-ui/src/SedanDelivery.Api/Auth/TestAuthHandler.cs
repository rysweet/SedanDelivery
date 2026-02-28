using System.Security.Claims;
using System.Text.Encodings.Web;
using Microsoft.AspNetCore.Authentication;
using Microsoft.Extensions.Options;

/// <summary>
/// Development-only authentication handler that accepts requests with the
/// <c>X-Test-Auth</c> header and creates a synthetic authenticated identity.
/// Enabled only when <c>ASPNETCORE_ENVIRONMENT=Development</c> AND
/// <c>E2E_TEST_AUTH=true</c>. Never available in production.
/// </summary>
public sealed class TestAuthHandler : AuthenticationHandler<AuthenticationSchemeOptions>
{
    public TestAuthHandler(
        IOptionsMonitor<AuthenticationSchemeOptions> options,
        ILoggerFactory logger,
        UrlEncoder encoder)
        : base(options, logger, encoder) { }

    protected override Task<AuthenticateResult> HandleAuthenticateAsync()
    {
        // Accept any request with the X-Test-Auth header
        if (!Request.Headers.TryGetValue("X-Test-Auth", out var testAuthValue)
            || string.IsNullOrEmpty(testAuthValue))
        {
            return Task.FromResult(AuthenticateResult.Fail("Missing X-Test-Auth header"));
        }

        var claims = new[]
        {
            new Claim(ClaimTypes.NameIdentifier, "e2e-test-user"),
            new Claim(ClaimTypes.Name, "E2E Test User"),
            new Claim(ClaimTypes.Email, "sedan-e2e-test@DefenderATEVET17.onmicrosoft.com"),
            new Claim("oid", "740c3fc5-1506-4ac9-80ae-f05e61692780"),
        };
        var identity = new ClaimsIdentity(claims, Scheme.Name);
        var principal = new ClaimsPrincipal(identity);
        var ticket = new AuthenticationTicket(principal, Scheme.Name);

        return Task.FromResult(AuthenticateResult.Success(ticket));
    }
}
