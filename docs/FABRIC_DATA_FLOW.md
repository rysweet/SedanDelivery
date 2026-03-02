# SedanDelivery → Microsoft Fabric Data Flow

This document describes the complete end-to-end data pipeline from
SedanDelivery (Valkey + GQL module) to Microsoft Fabric Graph for
analytics.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│ Client Applications                                                 │
│                                                                     │
│   redis-cli / Neo4j driver / any Redis client                       │
│       │ GQL.MUTATE / GQL.QUERY (port 6379, TCP)                     │
│       │ Bolt protocol (port 7687, TCP)                              │
└───────┼─────────────────────────────────────────────────────────────┘
        ▼
┌─────────────────────────────────────────────────────────────────────┐
│ Azure Container Apps Environment (sd2-{env}-cae)                    │
│ VNet: sd2-{env}-vnet (10.0.0.0/16), Subnet: /23                    │
│ External TCP ingress (ports 6379 + 7687)                            │
│                                                                     │
│  ┌──────────────────────────┐   ┌─────────────────────────────────┐ │
│  │ sd2-{env}-redis           │   │ sd2-{env}-fabric-sync           │ │
│  │ (Valkey 8.1 + GQL module) │   │ (Rust async sidecar)           │ │
│  │                           │   │                                 │ │
│  │ • GQL.MUTATE / GQL.QUERY │   │ • XREADGROUP on __sd2:changelog │ │
│  │ • Bolt v4.4 server       │   │ • Retries with backoff          │ │
│  │ • __sd2:changelog stream  │   │ • Graceful shutdown (SIGTERM)   │ │
│  │                           │   │                                 │ │
│  │ GQL_FABRIC_CHANGELOG=1   │   │ GQL_FABRIC_ENDPOINT=...         │ │
│  └───────────┬───────────────┘   └──────────┬──────────────────────┘ │
│              │ XADD __sd2:changelog          │                       │
│              └──────────────────────────────→│                       │
└─────────────────────────────────────────────┼───────────────────────┘
                                              │
        ┌─────────────────────────────────────┘
        │ sync-to-onelake.py (periodic or triggered)
        ▼
┌─────────────────────────────────────────────────────────────────────┐
│ Microsoft Fabric (workspace: SD2-{env})                             │
│                                                                     │
│  Step 1: Extract from Valkey                                        │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ GQL.QUERY default "MATCH (n) RETURN id(n), labels(n), ..."   │   │
│  │ GQL.QUERY default "MATCH ()-[r]->() RETURN id(r), ..."      │   │
│  └──────────────────────────────────────────────────────────────┘   │
│        │                                                            │
│        ▼                                                            │
│  Step 2: Write Parquet                                              │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ sd2_nodes.parquet — columns: node_id, node_labels,           │   │
│  │                     properties_json, (flattened props)        │   │
│  │ sd2_edges.parquet — columns: edge_id, edge_type,             │   │
│  │                     source_id, target_id, properties_json     │   │
│  └──────────────────────────────────────────────────────────────┘   │
│        │                                                            │
│        ▼                                                            │
│  Step 3: Upload to OneLake                                          │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ PUT https://onelake.dfs.fabric.microsoft.com/                │   │
│  │     {workspace}/{lakehouse}/Files/graph-import/sd2_nodes...  │   │
│  │ Auth: Bearer token (scope: storage.azure.com)                │   │
│  └──────────────────────────────────────────────────────────────┘   │
│        │                                                            │
│        ▼                                                            │
│  Step 4: LoadTable API                                              │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ POST /v1/workspaces/{ws}/lakehouses/{lh}/tables/{t}/load    │   │
│  │ → Converts Parquet to Delta tables                           │   │
│  │ Auth: Bearer token (scope: api.fabric.microsoft.com)         │   │
│  └──────────────────────────────────────────────────────────────┘   │
│        │                                                            │
│        ▼                                                            │
│  Step 5: Update GraphModel Definition                               │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ POST /v1/workspaces/{ws}/GraphModels/{gm}/updateDefinition  │   │
│  │ → Maps sd2_nodes → SD2Node (labels, properties)             │   │
│  │ → Maps sd2_edges → SD2Edge (source, target, properties)     │   │
│  └──────────────────────────────────────────────────────────────┘   │
│        │                                                            │
│        ▼                                                            │
│  Step 6: Refresh GraphModel                                         │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ POST /v1/workspaces/{ws}/GraphModels/{gm}/jobs/RefreshGraph │   │
│  │ → Ingests Delta tables into queryable graph                  │   │
│  └──────────────────────────────────────────────────────────────┘   │
│        │                                                            │
│        ▼                                                            │
│  Step 7: Query via GQL                                              │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ POST /v1/workspaces/{ws}/GraphModels/{gm}/executeQuery      │   │
│  │ Body: {"query": "MATCH (n:SD2Node) RETURN n LIMIT 10"}      │   │
│  │ → Returns typed JSON results                                 │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  ┌─────────────────┐  ┌─────────────────┐  ┌───────────────────┐   │
│  │ Lakehouse        │  │ GraphModel       │  │ GraphQuerySet     │   │
│  │ sd2lakehouse     │  │ sedangraph       │  │ (saved queries)   │   │
│  │                  │  │                  │  │                   │   │
│  │ Tables/          │  │ SD2Node          │  │ GQL queries       │   │
│  │  sd2_nodes       │  │ SD2Edge          │  │ for analytics     │   │
│  │  sd2_edges       │  │                  │  │                   │   │
│  └─────────────────┘  └─────────────────┘  └───────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

## Fabric Resources

| Resource | Type | Purpose |
|----------|------|---------|
| SD2-{env} | Workspace | Contains all Fabric items |
| sd2lakehouse | Lakehouse | Stores Parquet → Delta tables |
| sedangraph | GraphModel | Maps tables to graph schema |

## Data Tables

### sd2_nodes

| Column | Type | Description |
|--------|------|-------------|
| node_id | STRING | Node ID from SedanDelivery |
| node_labels | STRING | JSON array of labels (e.g., `["Person"]`) |
| properties_json | STRING | Full property map as JSON |
| *(flattened)* | STRING | Each property also as a separate column |

### sd2_edges

| Column | Type | Description |
|--------|------|-------------|
| edge_id | STRING | Edge ID from SedanDelivery |
| edge_type | STRING | Relationship type (e.g., `E2E_LINK`) |
| source_id | STRING | Source node ID |
| target_id | STRING | Target node ID |
| properties_json | STRING | Full property map as JSON |

## Authentication

Two separate OAuth2 token scopes are needed:

| Token | Scope | Used For |
|-------|-------|----------|
| OneLake token | `https://storage.azure.com/.default` | File upload to OneLake |
| Fabric API token | `https://api.fabric.microsoft.com/.default` | LoadTable, RefreshGraph, executeQuery |

Both use service principal client credentials flow.

## Provisioning

The `scripts/provision-fabric.sh` script creates:

1. **Workspace** (SD2-{env}) — auto-detects Fabric capacity
2. **Lakehouse** (sd2lakehouse) — for Delta table storage
3. **GraphModel** (sedangraph) — tries GraphModel type first, falls back to GraphQLApi

The script outputs:
```
FABRIC_ENDPOINT=https://api.fabric.microsoft.com/v1/workspaces/{ws}/GraphModels/{gm}
FABRIC_ITEM_TYPE=GraphModel
FABRIC_WORKSPACE_ID={ws}
FABRIC_LAKEHOUSE_ID={lh}
FABRIC_GDB_ID={gm}
```

## Sync Script

`scripts/sync-to-onelake.py` performs the full sync:

```bash
# Required environment variables
REDIS_HOST=sd2-main-redis.purplerock-b0286967.eastus2.azurecontainerapps.io
REDIS_PORT=6379
REDIS_PASSWORD=<valkey-password>
FABRIC_TENANT_ID=<tenant-id>
FABRIC_CLIENT_ID=<sp-app-id>
FABRIC_CLIENT_SECRET=<sp-secret>
FABRIC_WORKSPACE_ID=<workspace-id>
FABRIC_LAKEHOUSE_ID=<lakehouse-id>
FABRIC_GRAPHMODEL_ID=<graphmodel-id>

# Run
pip install pyarrow requests
python scripts/sync-to-onelake.py
```

## E2E Validation

The `.github/workflows/e2e-fabric.yml` workflow validates the full pipeline:

1. **Write** test data to Valkey via `redis-cli GQL.MUTATE`
2. **Sync** to OneLake via `sync-to-onelake.py`
3. **Validate** by querying the Fabric GraphModel via GQL Query API

## Changelog Stream

The GQL module writes change events to the `__sd2:changelog` Redis stream
when `GQL_FABRIC_CHANGELOG=1` is set. Each event is a JSON object:

```json
{"op":"create_node","graph":"default","id":10,"labels":["Person"],"properties":{"name":"Alice","age":30}}
```

The `sedandelivery-fabric-sync` sidecar reads these events via `XREADGROUP`
with a consumer group for reliable delivery. Currently, the sidecar attempts
to replay events against the Fabric API directly — the sync-to-onelake.py
script provides the alternative bulk-export path via Parquet.

## Known Limitations

- **GraphModel updateDefinition** requires user-delegated auth (SP not supported
  in preview) — the E2E workflow uses az CLI login which provides user context
- **Schema evolution** is not supported by Fabric Graph — structural changes
  require re-creating the GraphModel
- **Sync is batch, not real-time** — the sync script does a full snapshot export
  each run, not incremental replication
- **Testkit**: 117/118 (99.2%) — 1 skip due to Python pytz UTC/+00:00 ambiguity
  (server is correct, driver limitation)
