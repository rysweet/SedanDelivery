---
title: Graph Visualizer Deployment Guide
description: Build container images and deploy to Azure Container Apps with .NET Aspire
type: howto
updated: 2026-02-27
---

# Deployment Guide

This guide covers building container images for gql-api and SedanDelivery.Api,
generating an Aspire manifest, and deploying to Azure Container Apps.

## Contents

- [Architecture in Production](#architecture-in-production)
- [Prerequisites](#prerequisites)
- [Build Container Images](#build-container-images)
- [Generate the Aspire Manifest](#generate-the-aspire-manifest)
- [Deploy to Azure Container Apps](#deploy-to-azure-container-apps)
- [Environment Variables Reference](#environment-variables-reference)
- [Health Check Endpoints](#health-check-endpoints)
- [Monitoring and Observability](#monitoring-and-observability)

## Architecture in Production

In production the React SPA is served as static files from a CDN or storage
account. The two backend services are deployed as Azure Container Apps:

```
Browser (CDN / Static Web Apps)
  sedan-web (static HTML/JS/CSS)
       |
       | HTTPS  (VITE_API_URL → Container Apps ingress)
       v
sedan-api  (Azure Container App, port 5000)
  .NET 8 BFF — CORS, rate limiting, OpenTelemetry
       |
       | HTTP  (GQL_API_URL → internal Container Apps FQDN)
       v
gql-api    (Azure Container App, port 3001)
  Rust/Axum — gql-core InMemoryGraphStore
```

Both Container Apps are in the same Container Apps Environment and communicate
over the internal virtual network. gql-api does not need public ingress.

## Prerequisites

| Tool | Purpose |
|---|---|
| Docker or Podman | Build container images |
| Azure CLI (`az`) | Deploy to Azure |
| .NET 10 SDK | Generate the Aspire manifest |
| Rust toolchain | Build the gql-api binary |

## Build Container Images

### gql-api (Rust)

```dockerfile
# Dockerfile at the repository root (same directory as Cargo.toml)
FROM rust:1.77-slim AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p gql-api

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/gql-api /usr/local/bin/gql-api
EXPOSE 3001
CMD ["gql-api"]
```

Build and push:

```bash
# From the repository root
docker build -f Dockerfile.gql-api -t <registry>/gql-api:latest .
docker push <registry>/gql-api:latest
```

### SedanDelivery.Api (.NET 8)

```bash
cd sedan-graph-ui

# Publish a self-contained image using the dotnet SDK
docker build -f src/SedanDelivery.Api/Dockerfile \
  -t <registry>/sedan-api:latest \
  --build-arg TARGETARCH=amd64 .
docker push <registry>/sedan-api:latest
```

A minimal `Dockerfile` for the .NET API:

```dockerfile
FROM mcr.microsoft.com/dotnet/aspnet:10.0 AS base
WORKDIR /app
EXPOSE 5000

FROM mcr.microsoft.com/dotnet/sdk:10.0 AS build
WORKDIR /src
COPY ["src/SedanDelivery.Api/SedanDelivery.Api.csproj", "src/SedanDelivery.Api/"]
COPY ["src/SedanDelivery.ServiceDefaults/SedanDelivery.ServiceDefaults.csproj", "src/SedanDelivery.ServiceDefaults/"]
RUN dotnet restore "src/SedanDelivery.Api/SedanDelivery.Api.csproj"
COPY . .
RUN dotnet publish "src/SedanDelivery.Api/SedanDelivery.Api.csproj" \
    -c Release -o /app/publish --no-restore

FROM base AS final
WORKDIR /app
COPY --from=build /app/publish .
ENTRYPOINT ["dotnet", "SedanDelivery.Api.dll"]
```

### sedan-web (React SPA)

The SPA is compiled to static files and does not need a container:

```bash
cd sedan-graph-ui/src/sedan-web

# Set the BFF URL at build time
VITE_API_URL=https://sedan-api.<env>.azurecontainerapps.io \
  npm run build

# Output is in dist/
```

Deploy the `dist/` directory to Azure Static Web Apps or any CDN.

## Generate the Aspire Manifest

The Aspire AppHost can generate an Azure Container Apps manifest:

```bash
cd sedan-graph-ui

dotnet run --project src/SedanDelivery.AppHost \
  -- --publisher manifest \
  --output-path ./aspire-manifest.json
```

The manifest describes all services, their environment variables, and ingress
configuration. Review it before deployment — the React SPA entry is marked
`ExcludeFromManifest` and will not appear.

## Deploy to Azure Container Apps

### Create the environment

```bash
RESOURCE_GROUP=rg-sedan-delivery
LOCATION=westus2
ENV_NAME=sedan-env

az group create --name $RESOURCE_GROUP --location $LOCATION

az containerapp env create \
  --name $ENV_NAME \
  --resource-group $RESOURCE_GROUP \
  --location $LOCATION
```

### Deploy gql-api

gql-api does not need public ingress — it is accessed only by the BFF:

```bash
az containerapp create \
  --name gql-api \
  --resource-group $RESOURCE_GROUP \
  --environment $ENV_NAME \
  --image <registry>/gql-api:latest \
  --target-port 3001 \
  --ingress internal \
  --min-replicas 1 \
  --max-replicas 3 \
  --env-vars RUST_LOG=info
```

Note the internal FQDN assigned by Azure. It follows the pattern:
`gql-api.internal.<env-id>.<region>.azurecontainerapps.io`

### Deploy SedanDelivery.Api

```bash
GQL_API_FQDN="gql-api.internal.<env-id>.<region>.azurecontainerapps.io"
SPA_ORIGIN="https://<your-static-web-app>.azurestaticapps.net"

az containerapp create \
  --name sedan-api \
  --resource-group $RESOURCE_GROUP \
  --environment $ENV_NAME \
  --image <registry>/sedan-api:latest \
  --target-port 5000 \
  --ingress external \
  --min-replicas 1 \
  --max-replicas 5 \
  --env-vars \
    GQL_API_URL=http://$GQL_API_FQDN \
    CORS_ORIGINS=$SPA_ORIGIN \
    ASPNETCORE_ENVIRONMENT=Production
```

### Deploy sedan-web (Static Web Apps)

```bash
az staticwebapp create \
  --name sedan-web \
  --resource-group $RESOURCE_GROUP \
  --location $LOCATION \
  --source sedan-graph-ui/src/sedan-web/dist
```

Set `VITE_API_URL` at build time to the external FQDN of `sedan-api` before
running `npm run build`.

## Environment Variables Reference

### gql-api (Rust)

| Variable | Default | Description |
|---|---|---|
| `RUST_LOG` | `info` | Log level. Valid values: `trace`, `debug`, `info`, `warn`, `error` |

gql-api listens on `0.0.0.0:3001`. The port is not configurable without
recompiling. Map container port 3001 to the host or Container Apps ingress.

### SedanDelivery.Api (.NET)

| Variable | Default | Description |
|---|---|---|
| `GQL_API_URL` | `http://localhost:3001` | Full URL of the gql-api service |
| `CORS_ORIGINS` | _(empty)_ | Comma-separated list of allowed CORS origins in addition to `localhost:5173` and `localhost:3000` |
| `ASPNETCORE_ENVIRONMENT` | `Production` | Controls whether OpenAPI/Scalar UI is served (Development only) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | _(empty)_ | OTLP collector endpoint. When set, traces and metrics are exported via OTLP |
| `OTEL_SERVICE_NAME` | Application name | Overrides the service name in telemetry |

### sedan-web (React, build-time only)

| Variable | Default | Description |
|---|---|---|
| `VITE_API_URL` | _(empty)_ | Base URL for BFF API calls. When empty, requests go to the same origin (useful behind a reverse proxy). Set to the full BFF URL for cross-origin deployments. |

## Health Check Endpoints

The .NET BFF exposes two health check endpoints registered by `MapDefaultEndpoints`
from ServiceDefaults:

| Endpoint | Purpose | Response |
|---|---|---|
| `GET /health` | Readiness probe — all health checks | `{"status": "Healthy"}` |
| `GET /alive` | Liveness probe — self check only | `{"status": "Healthy"}` |

Configure Azure Container Apps probes:

```bash
# Liveness probe
az containerapp update \
  --name sedan-api \
  --resource-group $RESOURCE_GROUP \
  --set-env-vars ... \
  --liveness-probe-path /alive \
  --liveness-probe-port 5000 \
  --liveness-probe-initial-delay 5 \
  --liveness-probe-period 10

# Readiness probe
az containerapp update \
  --name sedan-api \
  --resource-group $RESOURCE_GROUP \
  --readiness-probe-path /health \
  --readiness-probe-port 5000
```

gql-api exposes `GET /health` at port 3001:

```json
{
  "status": "ok",
  "version": "0.1.0",
  "demo_graph_exists": false
}
```

Use this for the gql-api liveness probe.

## Monitoring and Observability

### OpenTelemetry (SedanDelivery.Api)

ServiceDefaults configures OpenTelemetry tracing and metrics automatically.
Instrumentation is enabled for:

- ASP.NET Core (HTTP request tracing and server metrics)
- HttpClient (outbound calls to gql-api)
- .NET runtime metrics (GC, thread pool, memory)

To export to Azure Monitor (Application Insights):

```bash
az containerapp update \
  --name sedan-api \
  --resource-group $RESOURCE_GROUP \
  --env-vars \
    OTEL_EXPORTER_OTLP_ENDPOINT=https://<ingest-endpoint>/v2/track \
    APPLICATIONINSIGHTS_CONNECTION_STRING=InstrumentationKey=...
```

To export to any OTLP-compatible collector (Grafana, Jaeger, etc.):

```bash
az containerapp update \
  --name sedan-api \
  --resource-group $RESOURCE_GROUP \
  --env-vars \
    OTEL_EXPORTER_OTLP_ENDPOINT=http://<collector>:4317
```

### Structured logging (gql-api)

gql-api emits structured JSON logs via `tracing-subscriber`. Set `RUST_LOG` to
control verbosity:

```bash
# Show all request traces
RUST_LOG=tower_http=debug,gql_api=debug

# Production default
RUST_LOG=info
```

Logs are written to stdout and captured by Azure Container Apps log streaming.
Query them in Log Analytics:

```kusto
ContainerAppConsoleLogs_CL
| where ContainerName_s == "gql-api"
| project TimeGenerated, Log_s
| order by TimeGenerated desc
```

### Correlation IDs

Every response from SedanDelivery.Api includes an `X-Request-Id` header. If the
incoming request already includes `X-Request-Id`, that value is echoed back.
Use this header to correlate browser requests with server-side traces.
