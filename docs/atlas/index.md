---
layout: default
title: Code Atlas
---

# SedanDelivery Code Atlas

A living architecture document derived from code-first truth. Last rebuilt: 2026-03-17.

> **Rendering note:** Diagrams are embedded as pre-rendered SVG images for reliable
> display on GitHub Pages. Mermaid source is preserved in collapsed
> `<details>` blocks for reference. The site also loads Mermaid JS via
> `_includes/head-custom.html` as a fallback for any inline mermaid blocks.

## Layer Overview

| Layer | Description | Key Diagrams |
|-------|-------------|-------------|
| [Repository Surface](repo-surface/) | Project structure, build systems, configuration | [repo-surface.svg](repo-surface/repo-surface.svg) |
| [Compile-time Dependencies](compile-deps/) | Cargo workspace dependency graph, external crates | [dependencies.svg](compile-deps/dependencies.svg) |
| [Runtime Topology](runtime-topology/) | Services, containers, ports, protocols | [topology.svg](runtime-topology/topology.svg) |
| [API Contracts](api-contracts/) | REST API routes, Bolt protocol, Redis commands | [routing.svg](api-contracts/routing.svg) |
| [Data Flow](data-flow/) | Query pipeline, mutation flow, Redis key schema | [dataflow.svg](data-flow/dataflow.svg) |
| [Service Components](service-components/) | Per-crate internal module architecture | 5 per-crate diagrams |
| [AST+LSP Bindings](ast-lsp-bindings/) | Cross-crate symbol references, dead code | [symbol-references.svg](ast-lsp-bindings/symbol-references.svg) |
| [User Journeys](user-journeys/) | 5 end-to-end paths from entry to outcome | 5 sequence diagrams |

## Quick Navigation

### By Question

| Question | Layer |
|----------|-------|
| What crates exist and how do they depend on each other? | [compile-deps](compile-deps/) |
| What services run in production? | [runtime-topology](runtime-topology/) |
| What API endpoints are available? | [api-contracts](api-contracts/) |
| How does a Cypher query flow through the system? | [data-flow](data-flow/) |
| What modules are inside the Bolt server? | [service-components](service-components/) |
| Are there any dead exported symbols? | [ast-lsp-bindings](ast-lsp-bindings/) |
| How does a Neo4j driver interact with SedanDelivery? | [user-journeys](user-journeys/) |

### By Role

| Role | Start Here |
|------|-----------|
| New engineer | [repo-surface](repo-surface/) → [runtime-topology](runtime-topology/) → [user-journeys](user-journeys/) |
| Backend developer | [compile-deps](compile-deps/) → [service-components](service-components/) → [data-flow](data-flow/) |
| DevOps / SRE | [runtime-topology](runtime-topology/) → [api-contracts](api-contracts/) |
| Security reviewer | [api-contracts](api-contracts/) → [ast-lsp-bindings](ast-lsp-bindings/) |

## Architecture Summary

SedanDelivery is a Rust openCypher query engine with:

- **3 query engines**: openCypher, Apache TinkerPop Gremlin, GQL (ISO/IEC 39075)
- **4 network interfaces**: REST API (axum), Bolt v4.4, Gremlin WebSocket, Redis module commands
- **7 storage backends**: InMemory, PersistentGraphStore (WAL), CachedGraphStore, Redis/Valkey, ModuleRedisStore, Microsoft Fabric, FanoutStore
- **Core pipeline**: Lexer → Parser → Validator → Planner (28+ operators) → Executor
- **17 workspace crates** with `cypher-core` as the foundational library
- **Azure-native deployment**: Container Apps, Bicep IaC, KEDA auto-scaling

## Staleness

This atlas should be rebuilt when:
- Cargo.toml dependency changes (compile-deps)
- docker-compose*.yml changes (runtime-topology)
- Route handler or protocol changes (api-contracts)
- New modules or crate restructuring (service-components)
