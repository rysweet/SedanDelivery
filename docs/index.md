---
layout: default
title: SedanDelivery Documentation
---

# SedanDelivery Documentation

A Rust GQL/Cypher query engine — a clean-room, MIT/Apache-2.0-licensed replacement for RedisGraph/FalkorDB.

## Guides

- [Tutorial](TUTORIAL) — Getting started with SedanDelivery
- [Research Notes](RESEARCH) — Security analysis and design rationale

## Architecture Decision Records

- [ADR-001: Storage Model](ADR-001-storage-model) — Redis-native storage design
- [ADR-002: Full GQL Compliance](ADR-002-full-gql-compliance) — Why we target the full spec
- [ADR-003: Parser Strategy](ADR-003-parser-strategy) — Hand-written recursive descent

## Deployment

SedanDelivery deploys to Azure Container Apps via GitHub Actions.

### Quick Deploy

1. Create a PR from your feature branch
2. Add the `deploy` label to the PR
3. The deployment workflow provisions an isolated Azure environment:
   - Azure Container Registry (ACR) for the Docker image
   - Container Apps Environment with persistent storage (Azure Files)
   - Valkey container with the GQL module loaded
4. A bot comment posts the Redis endpoint when deployment completes
5. Remove the `deploy` label or run the **Undeploy** workflow to clean up

### Infrastructure

| Resource | Purpose |
|----------|---------|
| Azure Container Registry | Stores the Docker image |
| Azure Storage Account + File Share | Persistent data volume for Valkey RDB/AOF |
| Container Apps Environment | Managed container hosting |
| Container App (Valkey + GQL module) | The deployed Redis with graph query engine |
| Azure Cache for Redis (optional) | Managed Redis backend (when `persistenceBackend=azure-cache`) |

### Storage Backends

SedanDelivery supports multiple storage backends via the `GraphStore` trait:

| Backend | Crate | Use Case |
|---------|-------|----------|
| `InMemoryGraphStore` | `gql-core` | Default in-module store, tests |
| `RedisGraphStore` | `gql-redis-store` | External Redis/Valkey via TCP |
| `FabricGraphStore` | `gql-fabric-store` | Microsoft Fabric Graph Database |
| `FanoutStore` | `gql-redis-store` | Write to multiple backends simultaneously |

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `GQL_REDIS_URL` | `redis://127.0.0.1:6379` | Redis connection URL for RedisGraphStore |
| `GQL_FABRIC_ENDPOINT` | — | Fabric Graph Database REST API URL |
| `GQL_FABRIC_TENANT_ID` | — | Azure AD tenant for Fabric auth |
| `GQL_FABRIC_CLIENT_ID` | — | Service principal app ID for Fabric |
| `GQL_FABRIC_CLIENT_SECRET` | — | Service principal secret for Fabric |
| `VALKEY_PASSWORD` | — | Password for Valkey authentication (injected at runtime) |

## Source Code

- [GitHub Repository](https://github.com/cloud-ecosystem-security/SedanDelivery)
