---
title: Getting Started with the Graph Visualizer
description: Prerequisites, local setup, and a first walkthrough
type: tutorial
updated: 2026-02-27
---

# Getting Started with the Graph Visualizer

This guide walks you through setting up the Graph Visualizer from scratch, running
all three services locally, and loading your first graph.

## Prerequisites

| Tool | Minimum version | Purpose |
|---|---|---|
| Rust toolchain (`cargo`) | 1.77 | Build and run gql-api |
| .NET SDK | 10.0 | Run SedanDelivery.Api and AppHost |
| Node.js | 20 LTS | Run the Vite dev server |
| npm | 10 | Install SPA dependencies |

Verify your environment:

```bash
cargo --version     # cargo 1.77.0 or later
dotnet --version    # 10.0.x or later
node --version      # v20.x.x or later
npm --version       # 10.x.x or later
```

## Clone the Repository

```bash
git clone https://github.com/cloud-ecosystem-security/sd3.git
cd sd3
```

All commands below assume the repository root as the working directory unless
noted otherwise.

## Install Dependencies

```bash
# Rust: fetch and compile all crates (first run takes 1-2 minutes)
cargo fetch

# Node: install SPA dependencies
cd sedan-graph-ui/src/sedan-web
npm install
cd ../../..
```

## Starting Locally

There are two ways to run the stack. Option 2 (Aspire) is recommended because it
streams logs from all services to a single dashboard.

### Option 1: Start Each Service Individually

Open three terminal windows.

**Terminal 1 — gql-api (Rust)**

```bash
# From the repository root
cargo run -p gql-api
# Output: INFO gql-api listening on 0.0.0.0:3001
```

**Terminal 2 — SedanDelivery.Api (.NET BFF)**

```bash
cd sedan-graph-ui
dotnet run --project src/SedanDelivery.Api
# Output: Now listening on: http://localhost:5000
```

**Terminal 3 — React SPA (Vite)**

```bash
cd sedan-graph-ui/src/sedan-web
npm run dev
# Output: Local: http://localhost:5173/
```

### Option 2: Start with Aspire (Recommended)

Aspire starts the .NET BFF automatically and streams structured logs from all
registered services to the Aspire dashboard at `http://localhost:15888`.

```bash
# Terminal 1 — gql-api must still be started separately
# (Aspire tracks it but does not build Rust binaries)
cargo run -p gql-api

# Terminal 2 — Aspire orchestrator
cd sedan-graph-ui
dotnet run --project src/SedanDelivery.AppHost

# Terminal 3 — React SPA
cd sedan-graph-ui/src/sedan-web
npm run dev
```

Open `http://localhost:15888` to see the Aspire dashboard with health status and
live logs for all services.

## Verifying the Stack

With all three services running, confirm each layer responds:

```bash
# gql-api health check
curl http://localhost:3001/health
# {"status":"ok","version":"0.1.0","demo_graph_exists":false}

# .NET BFF health check
curl http://localhost:5000/health
# {"status":"Healthy"}

# React SPA
open http://localhost:5173
```

If the BFF health check fails with a connection refused, confirm `GQL_API_URL` is
set correctly. In development, `appsettings.Development.json` defaults to
`http://localhost:3001`.

## First Walkthrough: Create and Explore a Graph

### 1. Create a graph

Open `http://localhost:5173`. The toolbar shows a graph selector dropdown. Click
the **+** icon beside it to open the "New Graph" dialog. Enter a name that matches
`^[a-zA-Z0-9_.\-]{1,64}$` — for example:

```
delivery-network
```

Click **Create**. The dropdown switches to `delivery-network` and the canvas shows
an empty graph.

### 2. Add nodes

Click the **Add Node** button in the toolbar (or press `N`). A dialog opens with
two fields:

- **Labels** — one or more labels separated by spaces, e.g. `Warehouse`
- **Properties** — comma-separated key:value pairs, e.g. `name:Seattle-HQ, capacity:5000`

Enter the following, then click **Create**:

```
Labels: Warehouse
Properties: name:Seattle-HQ, capacity:5000
```

Repeat to create a second node:

```
Labels: Warehouse
Properties: name:Portland-DC, capacity:3200
```

Both nodes appear on the canvas. The Legend Panel on the left shows a `Warehouse`
entry with a color swatch.

### 3. Add an edge

Click **Add Edge** in the toolbar (or press `E`). Select the source and target
nodes from the dropdowns, enter a relationship type:

```
Type: SUPPLIES
Properties: {"frequency": "daily"}
```

An arrow connects the two warehouse nodes.

### 4. Run a Cypher query

Click the **Query** button in the toolbar to open the Cypher editor. Type:

```cypher
MATCH (w:Warehouse)
RETURN w.name, w.capacity
ORDER BY w.capacity DESC
```

Press `Ctrl+Enter` to run. Results appear in a table below the editor showing:

```
w.name         w.capacity
Seattle-HQ     5000
Portland-DC    3200
```

### 5. Expand a neighborhood

Right-click one of the warehouse nodes on the canvas and select
**Expand Neighborhood**. Any nodes connected to that node are fetched and merged into
the canvas view.

### 6. Export the graph

Open the **Export** dialog from the toolbar menu. Click **Download JSON**. The
file contains all nodes and edges in the standard format:

```json
{
  "nodes": [
    {
      "id": "n1",
      "labels": ["Warehouse"],
      "properties": { "name": "Seattle-HQ", "capacity": 5000 }
    }
  ],
  "edges": [
    {
      "id": "e1",
      "type": "SUPPLIES",
      "source": "n1",
      "target": "n2",
      "properties": { "frequency": "daily" }
    }
  ]
}
```

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `GQL_API_URL` | `http://localhost:3001` | URL the .NET BFF uses to reach gql-api |
| `CORS_ORIGINS` | _(empty)_ | Comma-separated extra origins the BFF allows |
| `VITE_API_URL` | _(empty, same origin)_ | Base URL the SPA uses to reach the BFF |

In the Aspire AppHost, `GQL_API_URL` is injected automatically. When running
services individually, set it in `appsettings.Development.json` or as an
environment variable.

## Troubleshooting

**Canvas is blank after creating nodes**

The Sigma.js WebGL context requires hardware-accelerated rendering. Confirm your
browser has WebGL enabled at `about:gpu`. Chrome and Firefox both support WebGL by
default; some CI/CD environments may not.

**`cargo run -p gql-api` fails to compile**

Run `rustup update stable` to ensure your toolchain is current. The project
requires Rust 1.77 or later.

**BFF returns `503` on every request**

The BFF cannot reach gql-api. Check that `cargo run -p gql-api` is running and
that `GQL_API_URL` points to the correct address. When running inside Docker or
Aspire containers, `localhost` in the BFF refers to the container, not the host.

**Rate limit errors (HTTP 429)**

The `/query` and `/mutate` BFF endpoints allow 100 requests per 60-second window
per IP. Slow down automated scripts or run gql-api directly on port 3001.

## Next Steps

- [User Guide](./user-guide.md) — Full UI reference, keyboard shortcuts, schema view
- [API Reference](./api-reference.md) — Direct access to gql-api endpoints
- [Deployment](./deployment.md) — Build container images and deploy to Azure
