# Documentation Index

## Getting Started

**[TUTORIAL.md](TUTORIAL.md)** — How to use the gql-core library (code examples, usage guide)

## Architecture

**[PLAN.md](../PLAN.md)** — Full implementation plan, storage model, operator design

## Research

**[RESEARCH.md](RESEARCH.md)** — Investigation findings: RedisGraph/FalkorDB architecture, licensing analysis, crate selection rationale

## Graph Visualizer

Full-stack graph visualization SPA built on the gql-core engine. Includes a Rust
HTTP API, a .NET 8 BFF, and a React/Sigma.js SPA orchestrated by .NET Aspire.

- **[graph-visualizer/README.md](graph-visualizer/README.md)** — Overview, architecture diagram, component descriptions
- **[graph-visualizer/getting-started.md](graph-visualizer/getting-started.md)** — Prerequisites, local setup, first walkthrough
- **[graph-visualizer/user-guide.md](graph-visualizer/user-guide.md)** — UI reference: canvas, panels, Cypher editor, keyboard shortcuts
- **[graph-visualizer/api-reference.md](graph-visualizer/api-reference.md)** — Complete gql-api endpoint reference with request/response examples
- **[graph-visualizer/deployment.md](graph-visualizer/deployment.md)** — Build container images, generate Aspire manifest, deploy to Azure Container Apps
