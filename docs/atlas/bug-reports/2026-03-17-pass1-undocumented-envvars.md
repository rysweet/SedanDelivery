## Bug: 29 environment variables used in code but missing from docs/index.md

**Layer**: runtime-topology × api-contracts
**Severity**: High
**Pass**: 1
**Evidence**:

Compared `std::env::var("...")` references across all Rust source files against the
env var tables in `docs/index.md`. The following 29 env vars are read in code but
not documented in the main documentation page:

**Bolt server (sedandelivery-bolt/src/):**
- `BOLT_DRAIN_TIMEOUT_SECS` — graceful shutdown drain timeout
- `BOLT_HEALTH_BIND_ADDR` — health endpoint bind address
- `BOLT_HEALTH_PORT` — health endpoint port
- `BOLT_JSON_LOGS` — JSON-format log output
- `BOLT_MAX_CHUNK_SIZE` — max chunk size for Bolt framing
- `BOLT_MAX_DECODE_DEPTH` — max nesting depth for message decoding
- `BOLT_MAX_MESSAGE_SIZE` — max Bolt message size
- `BOLT_MAX_PENDING_RESULTS` — max buffered pending results
- `BOLT_SERVER_VERSION` — server version string sent to clients
- `BOLT_WS_MAX_WRITE_BUF` — WebSocket write buffer size

**REST API (gql-api/src/):**
- `GQL_API_DEFAULT_NEIGHBOR_LIMIT` — default neighbor exploration limit
- `GQL_API_DEFAULT_PAGE_LIMIT` — default page size
- `GQL_API_DRAIN_TIMEOUT_SECS` — graceful shutdown timeout
- `GQL_API_IMPORT_LIMIT` — max elements per import
- `GQL_API_JWKS_URL` — custom JWKS endpoint for JWT validation
- `GQL_API_MAX_EXPORT_ELEMENTS` — max elements per export
- `GQL_API_MAX_PAGE_LIMIT` — max page size
- `GQL_API_MAX_QUERY_LENGTH` — max query string length
- `GQL_API_MAX_SCHEMA_SCAN_ELEMENTS` — max schema scan elements
- `GQL_API_QUERY_BODY_LIMIT_BYTES` — max request body size for queries
- `GQL_API_TLS_CERT` — TLS certificate path
- `GQL_API_TLS_KEY` — TLS private key path
- `GQL_API_TLS_PORT` — TLS listen port
- `GQL_CSV_IMPORT_DIR` — CSV import directory

**Storage/Sync:**
- `GQL_FABRIC_CHANGELOG` — enable changelog stream
- `GQL_REDIS_BATCH_SIZE` — pipeline batch size
- `OTEL_EXPORTER_OTLP_ENDPOINT` — OpenTelemetry endpoint
- `REDIS_URL` — fallback for GQL_REDIS_URL
- `SYNC_DLQ_STREAM` — dead-letter queue stream name

code_quote (example, from `sedandelivery-bolt/src/main.rs`):
```
let max_conns_per_ip: usize = std::env::var("BOLT_MAX_CONN_PER_IP")
    .ok()
    .and_then(|v| v.parse().ok())
    .unwrap_or(50);
```

**Impact**: Operators cannot discover or configure these 29 env vars without reading source
code. Several are important for production deployments (TLS, drain timeouts, batch sizes,
OTEL endpoint).

**Fix**: Add all 29 env vars to the appropriate tables in `docs/index.md`, grouped by
component. Priority: TLS vars, drain timeout vars, and OTEL endpoint are most critical
for production operators.
