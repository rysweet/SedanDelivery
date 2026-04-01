---
title: Graph Visualizer
description: Interactive WebGL graph visualization SPA for SedanDelivery
updated: 2026-02-27
---

# Graph Visualizer

The Graph Visualizer is a browser-based tool for exploring, editing, and querying
property graphs stored in SedanDelivery's cypher-core engine. It renders graphs using
WebGL (Sigma.js v3) and handles datasets of 100,000+ nodes at interactive frame rates.

## Quick Start

Three commands start the full stack locally using .NET Aspire:

```bash
# 1. Start the Rust sd2-api (port 3001)
cargo run -p sd2-api

# 2. Start the Aspire orchestrator — launches the .NET BFF automatically
cd sedan-graph-ui && dotnet run --project src/SedanDelivery.AppHost

# 3. Start the React SPA (port 5173)
cd sedan-graph-ui/src/sedan-web && npm run dev
```

Open `http://localhost:5173` in your browser.

For full setup instructions including prerequisites, see
[Getting Started](./getting-started.md).

## Architecture

```
Browser (port 5173)
  sedan-web (React 18 + Vite)
       |
       | HTTP JSON
       v
SedanDelivery.Api (port 5000)     ← .NET 8 BFF
  - CORS, rate limiting
  - OpenAPI at /scalar/v1
       |
       | HTTP JSON (GQL_API_URL)
       v
sd2-api (port 3001)               ← Rust / Axum
  - REST CRUD for graphs, nodes, edges
  - Cypher query and mutate
  - Schema, neighbors, export/import
       |
       | in-process
       v
cypher-core InMemoryGraphStore       ← Rust library
  - Parse → Plan → Execute pipeline
  - One store instance per named graph
```

All services are orchestrated by `.NET Aspire` (`SedanDelivery.AppHost`), which
injects service URLs and streams logs from all processes to the Aspire dashboard.

## Components

### sd2-api (Rust, port 3001)

HTTP REST service built with Axum. Wraps the cypher-core library and exposes all
graph operations over JSON. Every named graph lives as an `InMemoryGraphStore`
instance inside a shared `Mutex<HashMap<String, InMemoryGraphStore>>`.

Key capabilities:
- Full CRUD for named graphs, nodes, and edges
- Cypher query (`POST /graphs/{name}/query`) and mutate (`POST /graphs/{name}/mutate`)
- Schema introspection (`GET /graphs/{name}/schema`)
- Neighbor expansion (`GET /graphs/{name}/neighbors/{nodeId}`)
- Bulk export/import in JSON format

See [API Reference](./api-reference.md) for the complete endpoint list.

### SedanDelivery.Api (.NET 10, port 5000)

Backend-for-Frontend (BFF) that proxies all requests to sd2-api with:
- CORS policy allowing the Vite dev server (`localhost:5173`) and any origins in
  the `CORS_ORIGINS` environment variable
- Sliding-window rate limiting: 100 requests per 60 seconds per IP on `/query`
  and `/mutate` endpoints (queue depth: 10)
- Correlation IDs on every response via the `X-Request-Id` header
- OpenAPI documentation at `/scalar/v1` (development only)
- Health endpoints at `/health` and `/alive` from Aspire ServiceDefaults

### SedanDelivery.AppHost (.NET Aspire)

Orchestrates all services as a single distributed application:
- Registers `sd2-api` as an `ExecutableResource` on port 3001
- Injects `GQL_API_URL=http://localhost:3001` into the .NET API
- Registers the Vite dev server as an `NpmApp` on port 5173 (excluded from
  the Azure manifest — the SPA is deployed separately or served as static files)

### sedan-web (React 18 + TypeScript, port 5173)

Single-page application built with Vite. Key dependencies:

| Library | Version | Purpose |
|---|---|---|
| Sigma.js + `@react-sigma/core` | v3 / v4 | WebGL graph rendering |
| Graphology | 0.25 | Graph data structure |
| graphology-layout-forceatlas2 | 0.10 | Force-directed layout |
| Zustand | 4.5 | Client state management |
| TanStack Query | 5.x | Server state, caching |
| Monaco Editor | 4.6 | Cypher query editor |
| Tailwind CSS | 3.4 | Styling |

## UI Overview

```
+--Toolbar------------------------------------------------------+
|  [Graph Selector]  [Select] [Add Node] [Add Edge]  [Layout]   |
|  [Search Bar]                              [Query] [Schema]    |
+--Legend-------+--------Canvas-----------------+--Inspector----+
|  Node labels  |                               |  Node/Edge    |
|  with color   |   WebGL graph canvas          |  properties   |
|  swatches     |   (Sigma.js + Graphology)     |  (editable)   |
|               |                               |               |
|  Visibility   |                               |  Labels list  |
|  toggles per  |                               |               |
|  label        |   [Context menu on            |               |
|               |    right-click]               |               |
+---------------+-------------------------------+---------------+
|  Status bar: graph name · node count · edge count · zoom      |
+---------------------------------------------------------------+
```

## Further Reading

- [Getting Started](./getting-started.md) — Prerequisites, local setup, first walkthrough
- [User Guide](./user-guide.md) — All UI features, keyboard shortcuts, Cypher editor
- [API Reference](./api-reference.md) — Complete sd2-api endpoint reference
- [Deployment](./deployment.md) — Azure Container Apps deployment with Aspire
