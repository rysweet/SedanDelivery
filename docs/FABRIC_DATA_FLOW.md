# SedanDelivery to Microsoft Fabric Data Flow

This document describes the complete, validated end-to-end data pipeline
from SedanDelivery (Valkey + GQL module) to Microsoft Fabric Graph
analytics. All steps have been tested and confirmed working in CI.

## How It Works

SedanDelivery is a graph query engine implemented as a Valkey/Redis
module. Clients write and read graph data via the Redis protocol
(GQL.MUTATE / GQL.QUERY commands). For analytics and visualization,
graph data is periodically synced to Microsoft Fabric where it becomes
queryable through the Fabric Graph Query API (which uses GQL, ISO/IEC 39075)
and the Fabric portal.

The sync is a batch process, not real-time streaming. A Python script
(`sync-to-onelake.py`) extracts all nodes and edges from Valkey, writes
them as Parquet files, uploads to a Fabric Lakehouse via the OneLake
storage API, and triggers the GraphModel to ingest the data.

## Architecture Diagram

```
                        CLIENT APPLICATIONS
                     (redis-cli, Neo4j driver)
                               |
                    GQL.MUTATE / GQL.QUERY
                         (port 6379 TCP)
                               |
                               v
    +----------------------------------------------------------+
    |           AZURE CONTAINER APPS ENVIRONMENT                |
    |              sd2-{env}-cae  (custom VNet)                 |
    |                                                          |
    |  +-------------------------+                             |
    |  |  sd2-{env}-redis        |    Valkey 8.1 + GQL module  |
    |  |                         |                             |
    |  |  GQL_FABRIC_CHANGELOG=1 |    Writes change events to  |
    |  |                         |    __sd2:changelog stream   |
    |  +-------------------------+                             |
    |                                                          |
    +----------------------------------------------------------+
                               |
                   sync-to-onelake.py
                  (triggered from CI or manually)
                               |
          +--------------------+--------------------+
          |                    |                    |
          v                    v                    v
    +-----------+      +--------------+     +----------------+
    | STEP 1    |      | STEP 2       |     | STEP 3         |
    | Extract   |      | Write        |     | Upload to      |
    | from      |----->| Parquet      |---->| OneLake        |
    | Valkey    |      | files        |     | (DataLake API) |
    +-----------+      +--------------+     +----------------+
                                                    |
                                                    v
    +----------------------------------------------------------+
    |             MICROSOFT FABRIC  (workspace SD2-{env})       |
    |                                                          |
    |  +---------------------------------------------------+   |
    |  | STEP 4: LoadTable API                              |   |
    |  | POST /lakehouses/{id}/tables/{name}/load           |   |
    |  | Converts Parquet --> Delta tables                   |   |
    |  +---------------------------------------------------+   |
    |                          |                                |
    |                          v                                |
    |  +---------------------+  +----------------------------+ |
    |  | Lakehouse           |  | STEP 5: updateDefinition   | |
    |  | sd2lakehouse        |  | Maps tables to graph types | |
    |  |                     |  |                            | |
    |  | Tables/sd2_nodes    |  | sd2_nodes --> SD2Node      | |
    |  | Tables/sd2_edges    |  | sd2_edges --> SD2Edge      | |
    |  +---------------------+  +----------------------------+ |
    |                          |                                |
    |                          v                                |
    |  +---------------------------------------------------+   |
    |  | STEP 6: RefreshGraph                               |   |
    |  | POST /GraphModels/{id}/jobs/RefreshGraph/instances |   |
    |  | Ingests Delta tables into queryable graph (~10s)   |   |
    |  +---------------------------------------------------+   |
    |                          |                                |
    |                          v                                |
    |  +---------------------------------------------------+   |
    |  | GraphModel: sedangraph                             |   |
    |  |                                                   |   |
    |  |  Node type: SD2Node                               |   |
    |  |    - node_id (PK), node_labels, properties_json   |   |
    |  |    - flattened properties (name, age, etc.)        |   |
    |  |                                                   |   |
    |  |  Edge type: SD2Edge                               |   |
    |  |    - source_id --> SD2Node.node_id                |   |
    |  |    - target_id --> SD2Node.node_id                |   |
    |  |    - properties_json                              |   |
    |  +---------------------------------------------------+   |
    |                          |                                |
    |                          v                                |
    |  +---------------------------------------------------+   |
    |  | STEP 7: GQL Query API                              |   |
    |  | POST /GraphModels/{id}/executeQuery?preview=true   |   |
    |  |                                                   |   |
    |  | MATCH (n:SD2Node) RETURN n.node_id, n.name        |   |
    |  | MATCH (a)-[r:SD2Edge]->(b) RETURN a, r, b         |   |
    |  +---------------------------------------------------+   |
    +----------------------------------------------------------+
```

## Step-by-Step Explanation

### Step 1: Extract from Valkey

The sync script connects to SedanDelivery via `redis-cli` and runs
GQL.QUERY commands to extract all nodes and edges:

```bash
# Nodes
GQL.QUERY default "MATCH (n) RETURN id(n) AS node_id, labels(n) AS node_labels, properties(n) AS props"

# Edges
GQL.QUERY default "MATCH (s)-[r]->(t) RETURN id(r) AS edge_id, type(r) AS edge_type, id(s) AS source_id, id(t) AS target_id, properties(r) AS props"
```

Each query returns one JSON object per line:
```json
{"node_id":10,"node_labels":["Person"],"props":{"name":"Alice","age":30}}
```

### Step 2: Write Parquet Files

The extracted data is written as Apache Parquet files using PyArrow.
Two files are produced:

**sd2_nodes.parquet:**

| Column | Type | Example |
|--------|------|---------|
| node_id | STRING | "10" |
| node_labels | STRING | '["Person"]' |
| properties_json | STRING | '{"name":"Alice","age":30}' |
| name | STRING | "Alice" |
| age | STRING | "30" |

Properties are stored both as a JSON blob (`properties_json`) and as
individual flattened columns for direct querying.

**sd2_edges.parquet:**

| Column | Type | Example |
|--------|------|---------|
| edge_id | STRING | "4" |
| edge_type | STRING | "KNOWS" |
| source_id | STRING | "11" |
| target_id | STRING | "12" |
| properties_json | STRING | '{"since":2020}' |

### Step 3: Upload to OneLake

Parquet files are uploaded to the Fabric Lakehouse via the OneLake
DataLake Storage API (Azure Data Lake Storage Gen2 compatible):

```
PUT  https://onelake.dfs.fabric.microsoft.com/{workspace}/{lakehouse}/Files/graph-import/sd2_nodes.parquet?resource=file
PATCH ...?action=append&position=0    (upload content)
PATCH ...?action=flush&position={len}  (finalize)
```

Authentication: Bearer token with scope `https://storage.azure.com/.default`

### Step 4: LoadTable API

The Fabric Lakehouse LoadTable API converts the uploaded Parquet files
into managed Delta tables:

```
POST /v1/workspaces/{ws}/lakehouses/{lh}/tables/sd2_nodes/load
Body: {"relativePath": "Files/graph-import/sd2_nodes.parquet", "pathType": "File", "mode": "Overwrite", "formatOptions": {"format": "Parquet"}}
```

This is an async operation (HTTP 202). The sync script polls the
Location header until the table is materialized (~30-45 seconds).

### Step 5: Update GraphModel Definition

The GraphModel definition maps the lakehouse Delta tables to graph
node and edge types. This is done via the `updateDefinition` API:

```
POST /v1/workspaces/{ws}/GraphModels/{gm}/updateDefinition
```

The definition has four parts (all base64-encoded JSON):

| Part | Purpose |
|------|---------|
| `.platform` | Metadata (type=GraphModel, displayName) |
| `graphType.json` | Schema: node/edge types, properties, primary keys |
| `graphDefinition.json` | Table mappings: which columns map to which properties |
| `dataSources.json` | Data sources: abfss:// paths to Delta tables |
| `stylingConfiguration.json` | Visual layout for the Fabric portal |

Data source paths use the OneLake abfss format:
```
abfss://{workspaceId}@onelake.dfs.fabric.microsoft.com/{lakehouseId}/Tables/sd2_nodes
```

### Step 6: Refresh GraphModel

After updating the definition, a RefreshGraph job ingests the Delta
tables into the graph engine:

```
POST /v1/workspaces/{ws}/GraphModels/{gm}/jobs/RefreshGraph/instances
```

The sync script polls the executeQuery endpoint until the graph
becomes queryable (typically ~10 seconds after refresh trigger).

### Step 7: Query via GQL

Once loaded, the graph is queryable via the GQL Query API:

```
POST /v1/workspaces/{ws}/GraphModels/{gm}/executeQuery?preview=true
Body: {"query": "MATCH (n:SD2Node) RETURN n.node_id, n.name LIMIT 10"}
```

Response:
```json
{
  "status": {"code": "00000", "description": "note: successful completion"},
  "result": {
    "kind": "TABLE",
    "columns": [
      {"name": "n.node_id", "gqlType": "STRING"},
      {"name": "n.name", "gqlType": "STRING"}
    ],
    "data": [
      {"n.node_id": "10", "n.name": "Alice"},
      {"n.node_id": "11", "n.name": "Bob"}
    ]
  }
}
```

Fabric Graph uses GQL (ISO/IEC 39075), which is syntactically similar
to openCypher but is the official international standard.

## Fabric Resources Created

| Resource | Fabric Type | Purpose |
|----------|-------------|---------|
| SD2-{env} | Workspace | Contains all items for this environment |
| sd2lakehouse | Lakehouse | Stores Parquet uploads and Delta tables |
| sedangraph | GraphModel | Maps Delta tables to a queryable graph |

These are created automatically by `scripts/provision-fabric.sh` during
deployment. The script auto-detects available Fabric capacity and tries
GraphModel type first (falls back to GraphQLApi if unavailable).

## Authentication

Two separate OAuth2 tokens are needed because OneLake and the Fabric
REST API use different resource scopes:

| Token | Scope | Used For |
|-------|-------|----------|
| OneLake | `https://storage.azure.com/.default` | Upload Parquet files |
| Fabric API | `https://api.fabric.microsoft.com/.default` | LoadTable, updateDefinition, RefreshGraph, executeQuery |

Both use service principal client credentials flow (client_id + secret).

## Tenant Requirements

The Fabric tenant must have the **"User can create Graph (preview)"**
setting enabled. This is configured in:

- Fabric Admin Portal -> Tenant settings -> "User can create Graph"
- Or via API: `POST /v1/admin/tenantsettings/ArtifactGraphPreview/update`

The GraphModel feature is available in East US 2 and many other regions.
See the [Fabric Graph region availability](https://learn.microsoft.com/en-us/fabric/graph/overview#region-availability).

## Running the Sync

### From CI (E2E Validation Workflow)

```bash
gh workflow run e2e-fabric.yml --field environment=main --field timeout=300
```

This runs the full pipeline: write test data -> sync -> validate.

### Manually

```bash
export REDIS_HOST=sd2-main-redis.purplerock-b0286967.eastus2.azurecontainerapps.io
export REDIS_PORT=6379
export REDIS_PASSWORD=<valkey-password>
export FABRIC_TENANT_ID=<tenant-id>
export FABRIC_CLIENT_ID=<sp-app-id>
export FABRIC_CLIENT_SECRET=<sp-secret>
export FABRIC_WORKSPACE_ID=<workspace-id>
export FABRIC_LAKEHOUSE_ID=<lakehouse-id>
export FABRIC_GRAPHMODEL_ID=<graphmodel-id>

pip install pyarrow requests
python scripts/sync-to-onelake.py
```

### Expected Output

```
============================================================
SedanDelivery -> OneLake Sync
============================================================

[EXTRACT] Reading graph data from Valkey...
  Nodes: 3, Edges: 1
  Format: Parquet

[AUTH] Acquiring tokens...
  Tokens acquired

[UPLOAD] Uploading to OneLake...
  Uploaded 1959 bytes -> Files/graph-import/sd2_nodes.parquet
  Uploaded 1474 bytes -> Files/graph-import/sd2_edges.parquet

[LOAD] Loading tables in Lakehouse...
  LoadTable sd2_nodes: completed (45s)
  LoadTable sd2_edges: completed (30s)
  Waiting 30s for Delta tables to materialize...

[SCHEMA] Updating GraphModel definition...
  GraphModel definition updated

[REFRESH] Triggering GraphModel refresh (waiting up to 120s)...
  GraphModel refresh triggered (job=925c4fba-...)
  GraphModel queryable after 10s
  Graph is loaded and queryable

============================================================
SUCCESS: Synced 3 nodes + 1 edges to OneLake
============================================================
```

## E2E Validation Results

The E2E validation (`.github/workflows/e2e-fabric.yml`) confirms:

```
[PASS] node_synced: SD2Node count in Fabric GraphModel: 3 (3.4s)
[PASS] edges_synced: SD2Edge count: 1 (1.6s)
[PASS] node_properties: First node: ['5', '["E2ESource"]'] (1.5s)
SUCCESS: All 3 checks passed
```

## Changelog Stream

The GQL module writes change events to **per-graph** Redis streams
when `GQL_FABRIC_CHANGELOG=1` is set. Each graph gets its own stream
key: `__sd2:changelog:{graphname}` (the `{graphname}` hash tag
ensures Redis Cluster slot affinity with the graph's data keys).

```
Stream key format:  __sd2:changelog:{<graphname>}
Example:            __sd2:changelog:{mydb}
```

Event payload (JSON in the `e` field of each stream entry):

```json
{"op":"create_node","graph":"mydb","id":10,"labels":["Person"],"properties":{"name":"Alice","age":30}}
```

The `sedandelivery-fabric-sync` sidecar discovers streams at startup
via `SMEMBERS __sd2_registry:graphs` and consumes them with a single
multi-stream `XREADGROUP` command for reliable delivery.

For the full changelog architecture -- stream discovery, multi-stream
XREADGROUP details, read-after-write session pinning, SSCAN threshold
behavior, and all related environment variables -- see the
[Auto-Scaling Guide](auto-scaling).

## Known Limitations

- **Batch sync, not real-time**: The sync script does a full snapshot
  export each run. For near-real-time sync, run it on a schedule (e.g.,
  every 5 minutes via cron or a Fabric Data Pipeline).

- **Schema is fixed**: Fabric Graph does not support schema evolution.
  If node/edge properties change, the GraphModel must be recreated.

- **Single node/edge type**: All nodes map to SD2Node and all edges to
  SD2Edge in the GraphModel. Original Valkey labels are preserved in
  the `node_labels` property for filtering.

- **Ephemeral container storage**: Valkey data does not persist across
  container restarts unless Azure Files is mounted. The sync to Fabric
  provides durable storage.

## File Reference

| File | Purpose |
|------|---------|
| `scripts/sync-to-onelake.py` | Full sync pipeline (extract -> Parquet -> OneLake -> LoadTable -> refresh) |
| `scripts/provision-fabric.sh` | Creates Fabric workspace, lakehouse, and GraphModel |
| `scripts/validate-fabric-e2e.py` | E2E validation (queries GraphModel via GQL) |
| `.github/workflows/e2e-fabric.yml` | CI workflow: write -> sync -> validate |
| `.github/workflows/deploy.yml` | Deploys infrastructure + triggers Fabric provisioning |
| `infra/main.bicep` | Azure infrastructure (VNet, ACR, Container Apps Environment) |
| `gql-fabric-store/src/lib.rs` | Rust Fabric API client (used by sidecar) |
| `sedandelivery-fabric-sync/src/main.rs` | Rust changelog consumer sidecar |
