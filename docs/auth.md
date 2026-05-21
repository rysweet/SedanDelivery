# Authentication

SedanDelivery supports authentication on multiple network interfaces:

| Interface | Auth Mechanism | Env Var | Details |
|-----------|---------------|---------|---------|
| REST API (`sd2-api`) | Microsoft Entra ID JWT | `GQL_API_ENTRA_TENANT_ID` + `GQL_API_ENTRA_CLIENT_ID` | [REST API Auth](#rest-api-entra-id) |
| Bolt (`sedandelivery-bolt`) | Password (basic auth) | `BOLT_PASSWORD` | [Bolt spec](specs/sedandelivery-bolt-spec) |
| Gremlin WebSocket (`sedandelivery-gremlin-server`) | Bearer token | `GREMLIN_AUTH_TOKEN` | [Gremlin WebSocket Auth](#gremlin-websocket-auth) |

All interfaces follow the same principle: **auth is opt-in**. If the corresponding env var is unset or empty, the interface accepts all connections (backward compatible). Set the env var to enforce authentication.

---

## Gremlin WebSocket Auth

The `sedandelivery-gremlin-server` WebSocket server supports configurable token-based authentication via the `GREMLIN_AUTH_TOKEN` environment variable.

### Quick start

```bash
# Set a strong token (generate one with: openssl rand -hex 32)
export GREMLIN_AUTH_TOKEN="my-secret-token"

# Start the server — auth is now enforced
cargo run -p sedandelivery-gremlin-server
```

All WebSocket connections must present the token. Without it, the server responds with HTTP 401 or closes the connection with a Gremlin error.

### How it works

Authentication uses a two-phase flow to support both header-capable clients (CLI tools, SDKs) and clients that cannot set WebSocket upgrade headers (browsers).

```
GREMLIN_AUTH_TOKEN unset/empty
  → Auth disabled. All connections accepted. (backward compatible)

GREMLIN_AUTH_TOKEN set:
  Phase 1 — WebSocket Upgrade (HTTP)
    Authorization header present + valid   → Accepted, authenticated
    Authorization header present + invalid → Rejected with HTTP 401
    Authorization header absent            → Accepted, auth deferred to Phase 2

  Phase 2 — First Message (WebSocket)
    (only if not authenticated in Phase 1)
    "bearer" field present + valid   → Authenticated, request processed normally
    "bearer" field present + invalid → Gremlin error response (code 401), connection closed
    "bearer" field absent            → Gremlin error response (code 401), connection closed
```

### Configuration

| Variable | Required | Description |
|----------|----------|-------------|
| `GREMLIN_AUTH_TOKEN` | No | Shared secret token. When set, all connections must present this token. When unset, auth is disabled. |

### Connecting with the Authorization header

Clients that can set HTTP headers on the WebSocket upgrade request should use the `Authorization: Bearer <token>` header. This is the preferred method — the connection is authenticated before any Gremlin messages are exchanged.

**Rust (tungstenite):**

```rust
use tungstenite::client::IntoClientRequest;

let mut request = "ws://localhost:8182/gremlin".into_client_request()?;
request.headers_mut().insert(
    "Authorization",
    format!("Bearer {}", token).parse()?,
);
let (ws, _) = tokio_tungstenite::connect_async(request).await?;
```

**Python (websockets):**

```python
import websockets

async with websockets.connect(
    "ws://localhost:8182/gremlin",
    additional_headers={"Authorization": f"Bearer {token}"}
) as ws:
    await ws.send('{"requestId":"...","op":"eval","processor":"","args":{"gremlin":"g.V().count()"}}')
    print(await ws.recv())
```

**curl (one-shot test):**

```bash
# Verify auth is enforced (expects 401 without token)
curl -i -N \
  -H "Connection: Upgrade" \
  -H "Upgrade: websocket" \
  -H "Sec-WebSocket-Version: 13" \
  -H "Sec-WebSocket-Key: $(openssl rand -base64 16)" \
  http://localhost:8182/gremlin
# → HTTP/1.1 401 Unauthorized

# With valid token
curl -i -N \
  -H "Connection: Upgrade" \
  -H "Upgrade: websocket" \
  -H "Sec-WebSocket-Version: 13" \
  -H "Sec-WebSocket-Key: $(openssl rand -base64 16)" \
  -H "Authorization: Bearer ${GREMLIN_AUTH_TOKEN}" \
  http://localhost:8182/gremlin
# → HTTP/1.1 101 Switching Protocols
```

### Connecting with first-message bearer field

Clients that cannot set HTTP headers (e.g., the browser `WebSocket` API) authenticate by including a `bearer` field in the first Gremlin request message:

```json
{
  "requestId": "cb89a7c0-1234-5678-9abc-def012345678",
  "op": "eval",
  "processor": "",
  "args": {
    "gremlin": "g.V().count()"
  },
  "bearer": "my-secret-token"
}
```

If the token is valid, the request is processed normally and the connection remains authenticated for subsequent messages. If invalid or missing, the server responds with a Gremlin error (status code 401) and closes the connection.

**JavaScript (browser):**

```javascript
const ws = new WebSocket("ws://localhost:8182/gremlin");
ws.onopen = () => {
  ws.send(JSON.stringify({
    requestId: crypto.randomUUID(),
    op: "eval",
    processor: "",
    args: { gremlin: "g.V().count()" },
    bearer: "my-secret-token"
  }));
};
ws.onmessage = (e) => console.log(JSON.parse(e.data));
```

### Security considerations

- **Use TLS in production.** The token is transmitted in plaintext over the wire. Always terminate TLS at a load balancer or reverse proxy in front of the Gremlin server.
- **Constant-time comparison.** Token validation uses a constant-time equality check to prevent timing-based side-channel attacks.
- **Tokens are never logged.** The server logs auth outcomes (success/failure) but never the token value itself.
- **No rate limiting.** The server does not rate-limit authentication attempts. Use a reverse proxy or firewall to limit connection rates in production.
- **Token rotation.** Changing the token requires restarting the server. All existing connections remain authenticated until they disconnect.

### Deployment

#### Docker / Container Apps

Set `GREMLIN_AUTH_TOKEN` as an environment variable or a secret reference:

```bash
# Docker
docker run -e GREMLIN_AUTH_TOKEN=my-secret-token \
  ghcr.io/cloud-ecosystem-security/sedandelivery:latest \
  /usr/local/bin/sedandelivery-gremlin-server

# Azure Container Apps (using secretref)
az containerapp update \
  --name sd2-prod-gremlin \
  --resource-group rg-sd2-prod \
  --set-env-vars "GREMLIN_AUTH_TOKEN=secretref:gremlin-auth-token"
```

#### Local development

For local development, simply do not set `GREMLIN_AUTH_TOKEN`:

```bash
# No auth — all connections accepted
cargo run -p sedandelivery-gremlin-server
```

### Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| HTTP 401 on WebSocket upgrade | `Authorization` header present but token is wrong | Check token matches `GREMLIN_AUTH_TOKEN` |
| Gremlin error 401 after connect | First message missing `bearer` field or wrong value | Include `bearer` field with correct token |
| All connections rejected | `GREMLIN_AUTH_TOKEN` set to wrong value | Verify the env var value on the server |
| Auth seems disabled | `GREMLIN_AUTH_TOKEN` is empty string or unset | Set a non-empty value to enable auth |
| Works in CLI but not in browser | Browser `WebSocket` API cannot set headers | Use the first-message `bearer` field instead |

---

## REST API (Entra ID)

The `sd2-api` REST API supports [Microsoft Entra ID](https://learn.microsoft.com/entra/identity/) (formerly Azure AD) JWT authentication.

### Quick start

```bash
# Set your Entra ID app registration details
export GQL_API_ENTRA_TENANT_ID="your-tenant-id"
export GQL_API_ENTRA_CLIENT_ID="your-client-id"

# Start the API — auth is now enforced
cargo run -p sd2-api
```

All requests except `GET /health` require a valid `Authorization: Bearer <jwt>` header.

### How it works

1. At startup, `sd2-api` fetches JWKS signing keys from `https://login.microsoftonline.com/{tenant}/discovery/v2.0/keys`
2. Every request (except `/health`) must include an `Authorization: Bearer <token>` header
3. The middleware validates:
   - **Signature**: RS256 against the JWKS key matching the token's `kid`
   - **Audience**: `<client_id>` or `api://<client_id>` (both Entra v1 and v2 formats)
   - **Issuer**: `https://login.microsoftonline.com/{tenant}/v2.0` or `https://sts.windows.net/{tenant}/`
   - **Expiry**: Token must not be expired
4. Invalid or missing tokens receive `401 Unauthorized` with a `WWW-Authenticate: Bearer` header

### Configuration

| Variable | Required | Description |
|----------|----------|-------------|
| `GQL_API_ENTRA_TENANT_ID` | Yes (prod) | Your Azure AD / Entra ID tenant ID |
| `GQL_API_ENTRA_CLIENT_ID` | Yes (prod) | App registration client ID (used as audience) |

When both variables are set, authentication is enforced. When either is missing, the middleware is a no-op (unauthenticated access allowed for local development).

When `ENVIRONMENT=production`, both variables are **required** — the API will refuse to start without them.

### Azure app registration setup

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

### Development mode

For local development, simply do not set the Entra env vars:

```bash
# No auth — all requests allowed
cargo run -p sd2-api
```

### Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `401 Unauthorized` on all requests | Token expired or wrong audience | Check `aud` claim matches `GQL_API_ENTRA_CLIENT_ID` |
| API panics at startup with "failed to fetch JWKS" | Network issue reaching `login.microsoftonline.com` | Check DNS, proxy, and firewall settings |
| API panics with "JWKS response contains no keys" | Tenant ID is wrong | Verify `GQL_API_ENTRA_TENANT_ID` |
| `401` with valid token | Key rotation — JWKS cached at startup | Restart the API to refresh keys |
