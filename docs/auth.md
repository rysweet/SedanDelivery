# Authentication

The `sd2-api` REST API supports [Microsoft Entra ID](https://learn.microsoft.com/entra/identity/) (formerly Azure AD) JWT authentication.

## Quick start

```bash
# Set your Entra ID app registration details
export GQL_API_ENTRA_TENANT_ID="your-tenant-id"
export GQL_API_ENTRA_CLIENT_ID="your-client-id"

# Start the API — auth is now enforced
cargo run -p sd2-api
```

All requests except `GET /health` require a valid `Authorization: Bearer <jwt>` header.

## How it works

1. At startup, `sd2-api` fetches JWKS signing keys from `https://login.microsoftonline.com/{tenant}/discovery/v2.0/keys`
2. Every request (except `/health`) must include an `Authorization: Bearer <token>` header
3. The middleware validates:
   - **Signature**: RS256 against the JWKS key matching the token's `kid`
   - **Audience**: `<client_id>` or `api://<client_id>` (both Entra v1 and v2 formats)
   - **Issuer**: `https://login.microsoftonline.com/{tenant}/v2.0` or `https://sts.windows.net/{tenant}/`
   - **Expiry**: Token must not be expired
4. Invalid or missing tokens receive `401 Unauthorized` with a `WWW-Authenticate: Bearer` header

## Configuration

| Variable | Required | Description |
|----------|----------|-------------|
| `GQL_API_ENTRA_TENANT_ID` | Yes (prod) | Your Azure AD / Entra ID tenant ID |
| `GQL_API_ENTRA_CLIENT_ID` | Yes (prod) | App registration client ID (used as audience) |

When both variables are set, authentication is enforced. When either is missing, the middleware is a no-op (unauthenticated access allowed for local development).

When `ENVIRONMENT=production`, both variables are **required** — the API will refuse to start without them.

## Azure app registration setup

1. Go to [Azure Portal > App registrations](https://portal.azure.com/#view/Microsoft_AAD_RegisteredApps)
2. Click **New registration**
3. Name: `sedan-sd2-api` (or your choice)
4. Supported account types: **Single tenant** (recommended)
5. Click **Register**
6. Copy the **Application (client) ID** → set as `GQL_API_ENTRA_CLIENT_ID`
7. Copy the **Directory (tenant) ID** → set as `GQL_API_ENTRA_TENANT_ID`

### Obtaining a token (for testing)

```bash
# Client credentials flow (service-to-service)
curl -X POST "https://login.microsoftonline.com/${GQL_API_ENTRA_TENANT_ID}/oauth2/v2.0/token" \
  -d "grant_type=client_credentials" \
  -d "client_id=${YOUR_APP_CLIENT_ID}" \
  -d "client_secret=${YOUR_APP_CLIENT_SECRET}" \
  -d "scope=api://${GQL_API_ENTRA_CLIENT_ID}/.default"

# Use the access_token in requests
curl -H "Authorization: Bearer ${ACCESS_TOKEN}" http://localhost:3001/graphs
```

## Development mode

For local development, simply do not set the Entra env vars:

```bash
# No auth — all requests allowed
cargo run -p sd2-api
```

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `401 Unauthorized` on all requests | Token expired or wrong audience | Check `aud` claim matches `GQL_API_ENTRA_CLIENT_ID` |
| API panics at startup with "failed to fetch JWKS" | Network issue reaching `login.microsoftonline.com` | Check DNS, proxy, and firewall settings |
| API panics with "JWKS response contains no keys" | Tenant ID is wrong | Verify `GQL_API_ENTRA_TENANT_ID` |
| `401` with valid token | Key rotation — JWKS cached at startup | Restart the API to refresh keys |
