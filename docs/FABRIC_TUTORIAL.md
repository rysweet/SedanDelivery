# Fabric Graph Replication Tutorial

This tutorial walks through deploying SedanDelivery with Fabric Graph replication,
writing graph data, viewing it in the Microsoft Fabric portal, querying via the
Fabric REST API, and monitoring the replication pipeline.

For the complete technical data flow documentation, see
[FABRIC_DATA_FLOW.md](FABRIC_DATA_FLOW.md).

## Contents

- [Prerequisites](#prerequisites)
- [1. Deploy SedanDelivery with Fabric Graph](#1-deploy-sedandelivery-with-fabric-graph)
- [2. Write graph data](#2-write-graph-data)
- [3. Sync to Fabric via OneLake](#3-sync-to-fabric-via-onelake)
- [4. Query Fabric Graph via GQL API](#4-query-fabric-graph-via-sd2-api)
- [5. Monitor replication](#5-monitor-replication)
- [Troubleshooting](#troubleshooting)

## Prerequisites

- Azure subscription with Contributor access
- GitHub repository secrets configured (`AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`,
  `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`) -- see [Secrets Configuration](secrets)
- Microsoft Fabric capacity provisioned in your Azure tenant
- The service principal (`AZURE_CLIENT_ID`) must have access to create Fabric workspaces

## 1. Deploy SedanDelivery with Fabric Graph

Fabric Graph replication is **enabled by default** (`ENABLE_FABRIC: 'true'` in
`deploy.yml`). No extra configuration is needed.

**Option A -- Deploy via PR label:**

```bash
# Create a feature branch and open a PR
git checkout -b my-feature
git push -u origin my-feature
gh pr create --title "my feature" --body "test deploy"

# Add the deploy label to trigger provisioning
gh pr edit --add-label deploy
```

**Option B -- Deploy via workflow dispatch:**

```bash
gh workflow run deploy.yml -f environment=my-env
```

The deployment workflow will:

1. Provision Azure infrastructure (ACR, Container Apps Environment, storage)
2. Run `scripts/provision-fabric.sh` to create a Fabric workspace named
   **SD2-{env}** with a Lakehouse (**sd2lakehouse**) and GraphModel (**sedangraph**)
3. Build and deploy the main Valkey+GQL container (`sd2-{env}-redis`)
   with `GQL_FABRIC_CHANGELOG=1` to enable changelog writes
4. Build and deploy the `sedandelivery-fabric-sync` sidecar as the
   `sd2-{env}-sync` Container App, which reads the changelog stream

Data is synced to Fabric via `scripts/sync-to-onelake.py` which exports
graph data as Parquet, uploads to the Lakehouse via OneLake, and refreshes
the GraphModel. See [FABRIC_DATA_FLOW.md](FABRIC_DATA_FLOW.md) for the
complete pipeline.

The executable and image name remain `sedandelivery-fabric-sync`. The live
Azure Container App resource name is `sd2-{env}-sync`.

A bot comment on the PR will post the Redis endpoint when deployment completes
(approximately 8 minutes).

### Find the deployed sync app

Use the Azure resource name when you inspect the live deployment:

```bash
ENV_NAME="main"

az containerapp show \
  --name "sd2-${ENV_NAME}-sync" \
  --resource-group "rg-sd2-${ENV_NAME}" \
  --query "{name:name,image:properties.template.containers[0].image}" \
  -o json
```

The reported image will still be `sedandelivery-fabric-sync:<tag>`. The Container
App resource remains `sd2-${ENV_NAME}-sync`.

## 2. Write graph data

Write data through either the Bolt protocol or the Redis protocol. Both paths write
to the Valkey changelog stream, which the sync sidecar replicates to Fabric.

### Via Bolt protocol (recommended)

```python
from neo4j import GraphDatabase

driver = GraphDatabase.driver("bolt://<your-endpoint>:7687", auth=None)
with driver.session() as session:
    session.run("CREATE (:Person {name: 'Alice', age: 30})")
    session.run("CREATE (:Person {name: 'Bob', age: 25})")
    session.run(
        "MATCH (a:Person {name: 'Alice'}), (b:Person {name: 'Bob'}) "
        "CREATE (a)-[:KNOWS {since: 2020}]->(b)"
    )
    session.run("CREATE (:Company {name: 'Acme', founded: 2020})")
    session.run(
        "MATCH (a:Person {name: 'Alice'}), (c:Company {name: 'Acme'}) "
        "CREATE (a)-[:WORKS_AT {since: 2022}]->(c)"
    )
driver.close()
```

### Via Redis protocol

```bash
redis-cli -h <your-endpoint> -p 6379 -a <valkey-password>

GQL.CREATE-GRAPH social
GQL.MUTATE social "CREATE (:Person {name: 'Alice', age: 30})"
GQL.MUTATE social "CREATE (:Person {name: 'Bob', age: 25})"
GQL.MUTATE social "MATCH (a:Person {name: 'Alice'}), (b:Person {name: 'Bob'}) CREATE (a)-[:KNOWS {since: 2020}]->(b)"
GQL.MUTATE social "CREATE (:Company {name: 'Acme', founded: 2020})"
GQL.MUTATE social "MATCH (a:Person {name: 'Alice'}), (c:Company {name: 'Acme'}) CREATE (a)-[:WORKS_AT {since: 2022}]->(c)"
```

### Via Python SDK

```python
from sedandelivery import SedanDB

db = SedanDB(host="<your-endpoint>", port=6379, password="<valkey-password>")
g = db.select_graph("social")
g.create()
g.mutate("CREATE (:Person {name: $name, age: $age})", name="Alice", age=30)
g.mutate("CREATE (:Person {name: $name, age: $age})", name="Bob", age=25)
g.mutate(
    "MATCH (a:Person {name: $a}), (b:Person {name: $b}) "
    "CREATE (a)-[:KNOWS {since: $yr}]->(b)",
    a="Alice", b="Bob", yr=2020
)
```

## 3. View replicated data in the Fabric portal

After writing data, the sync sidecar replicates it to Fabric within seconds
(configurable batch interval via `SD2_SYNC_BLOCK_MS`, default 1000 ms).

### Navigate to the workspace

1. Open [app.fabric.microsoft.com](https://app.fabric.microsoft.com)
2. In the left navigation, select **Workspaces**
3. Find and open **SD2-{env}** (e.g., `SD2-my-feature`)

### Open the GraphModel

1. Inside the workspace, locate the **sedangraph** item (type: GraphModel)
2. Select it to open the graph explorer

### Browse nodes and relationships

The graph explorer displays nodes and relationships visually. You can:

- Filter by label (e.g., `Person`, `Company`)
- Inspect properties on individual nodes and edges
- Expand relationship paths from a selected node

### Run openCypher queries in the Fabric query editor

Select the **Query** tab in the graph explorer to open the query editor. Run
standard openCypher queries:

```cypher
-- Find all people and their connections
MATCH (p:Person)-[r]->(target)
RETURN p.name AS person, type(r) AS relationship, labels(target) AS target_labels

-- Aggregation across the replicated graph
MATCH (p:Person)
RETURN avg(p.age) AS avg_age, count(p) AS total_people

-- Path queries
MATCH path = (a:Person)-[:KNOWS*1..3]->(b:Person)
RETURN a.name, b.name, length(path)
```

## 4. Query Fabric GraphModel via REST API

You can query the Fabric GraphModel programmatically using the Fabric REST API.

### Authentication

Obtain an Azure AD access token for the Fabric API scope:

```bash
# Using the Azure CLI
az account get-access-token \
  --resource "https://api.fabric.microsoft.com" \
  --query accessToken -o tsv
```

```python
# Using the Azure Identity library
from azure.identity import ClientSecretCredential

credential = ClientSecretCredential(
    tenant_id="<your-tenant-id>",
    client_id="<your-client-id>",
    client_secret="<your-client-secret>"
)
token = credential.get_token("https://api.fabric.microsoft.com/.default")
```

### Query endpoint URL

The deployed sync Container App stores `GQL_FABRIC_ENDPOINT` as the Fabric
GraphModel base URL:

```
https://api.fabric.microsoft.com/v1/workspaces/{workspace-id}/GraphModels/{graphmodel-id}
```

The live validator reuses that same value by exporting it as `FABRIC_ENDPOINT`
and posting queries to:

```
https://api.fabric.microsoft.com/v1/workspaces/{workspace-id}/GraphModels/{graphmodel-id}/executeQuery?preview=true
```

This GraphModel URL is the current deployed contract. The validator still keeps
compatibility handling for older endpoint families during rollout, but new
deployments should reuse the `GQL_FABRIC_ENDPOINT` value from `sd2-{env}-sync`.

The `provision-fabric.sh` script outputs these IDs during deployment. You can
also find the workspace ID in the Fabric portal URL.

### Example REST API calls

**List workspaces** to find your workspace ID:

```bash
TOKEN=$(az account get-access-token --resource "https://api.fabric.microsoft.com" --query accessToken -o tsv)

curl -s -H "Authorization: Bearer ${TOKEN}" \
  "https://api.fabric.microsoft.com/v1/workspaces" \
  | python3 -m json.tool
```

**List items in the workspace** to find the GraphModel ID:

```bash
WORKSPACE_ID="<your-workspace-id>"

curl -s -H "Authorization: Bearer ${TOKEN}" \
  "https://api.fabric.microsoft.com/v1/workspaces/${WORKSPACE_ID}/items?type=GraphModel" \
  | python3 -m json.tool
```

**Query the GraphModel:**

```bash
GRAPHMODEL_ID="<your-graphmodel-id>"

curl -s -X POST \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  "https://api.fabric.microsoft.com/v1/workspaces/${WORKSPACE_ID}/GraphModels/${GRAPHMODEL_ID}/executeQuery?preview=true" \
  -d '{"query": "MATCH (p:Person) RETURN p.name, p.age"}' \
  | python3 -m json.tool
```

**Full Python example:**

```python
import requests
from azure.identity import ClientSecretCredential

# Authenticate
credential = ClientSecretCredential(
    tenant_id="<your-tenant-id>",
    client_id="<your-client-id>",
    client_secret="<your-client-secret>"
)
token = credential.get_token("https://api.fabric.microsoft.com/.default").token

# Query the graph
workspace_id = "<your-workspace-id>"
graphmodel_id = "<your-graphmodel-id>"
url = (
    "https://api.fabric.microsoft.com/v1/workspaces/"
    f"{workspace_id}/GraphModels/{graphmodel_id}/executeQuery?preview=true"
)

response = requests.post(
    url,
    headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
    json={"query": "MATCH (p:Person)-[:WORKS_AT]->(c:Company) RETURN p.name AS person, c.name AS company"}
)
response.raise_for_status()
payload = response.json()

for row in payload["result"]["data"]:
    person = row["person"]
    company = row["company"]
    if isinstance(person, dict) and "value" in person:
        person = person["value"]
    if isinstance(company, dict) and "value" in company:
        company = company["value"]
    print(person, company)
```

### Reuse the deployed endpoint with the validator

The deploy workflow injects `GQL_FABRIC_ENDPOINT` into the `sd2-{env}-sync`
Container App. When you run `scripts/validate-fabric-e2e.py` locally, export
that same value as `FABRIC_ENDPOINT`:

```bash
ENV_NAME="my-feature"
RG="rg-sd2-${ENV_NAME}"

export FABRIC_ENDPOINT="$(az containerapp show \
  --name "sd2-${ENV_NAME}-sync" \
  --resource-group "${RG}" \
  --query "properties.template.containers[0].env[?name=='GQL_FABRIC_ENDPOINT'].value | [0]" \
  -o tsv)"
export BOLT_URI="bolt://$(az containerapp show \
  --name "sd2-${ENV_NAME}-bolt" \
  --resource-group "${RG}" \
  --query "properties.configuration.ingress.fqdn" \
  -o tsv):7687"
export FABRIC_TENANT_ID="<tenant-id>"
export FABRIC_CLIENT_ID="<client-id>"
export FABRIC_CLIENT_SECRET="<client-secret>"

python scripts/validate-fabric-e2e.py \
  --timeout 180 \
  --report fabric-e2e-report.json
```

## 5. Monitor replication

### Check the sync sidecar logs

View the `sd2-{env}-sync` Container App logs in Azure:

```bash
ENV_NAME="my-feature"

az containerapp logs show \
  --name "sd2-${ENV_NAME}-sync" \
  --resource-group "rg-sd2-${ENV_NAME}" \
  --follow
```

Healthy output shows batches being read from the changelog and written to Fabric:

```
[INFO] Connected to redis://sd2-my-feature-redis...internal:6379
[INFO] Fabric endpoint: https://api.fabric.microsoft.com/v1/workspaces/.../GraphModels/...
[INFO] Batch 1: 5 events synced (12ms)
[INFO] Batch 2: 3 events synced (8ms)
```

### Verify events in the changelog stream

Connect to the Valkey instance and inspect the `__sd2:changelog` stream:

```bash
redis-cli -h <your-endpoint> -p 6379 -a <valkey-password>

# Count pending events
XLEN __sd2:changelog

# Read the most recent 5 events
XREVRANGE __sd2:changelog + - COUNT 5

# Read all events from the beginning
XRANGE __sd2:changelog - +
```

Each changelog entry contains the graph name, operation type (`CREATE_NODE`,
`CREATE_EDGE`, `SET_PROPERTY`, etc.), and the full mutation payload.

### Check sync sidecar status

```bash
# Verify the sidecar container is running
az containerapp show \
  --name "sd2-${ENV_NAME}-sync" \
  --resource-group "rg-sd2-${ENV_NAME}" \
  --query "properties.runningStatus" -o tsv
```

## Troubleshooting

### Fabric workspace not created

**Symptom:** The deploy workflow completes but the `Provision Fabric Graph Database`
step shows `has_fabric=false`.

**Cause:** The service principal may lack permission to create Fabric workspaces,
or Fabric capacity is not provisioned in the tenant.

**Fix:**
1. Verify the service principal has Fabric workspace creation rights in the
   [Fabric admin portal](https://app.fabric.microsoft.com/admin-portal)
2. Confirm a Fabric capacity (F2 or higher) is provisioned in the Azure subscription
3. Re-run the deploy workflow

### Sync sidecar not replicating

**Symptom:** Data written to Valkey does not appear in the Fabric portal.

**Cause:** The sidecar may not be running, or it cannot reach the Fabric endpoint.

**Fix:**
1. Check sidecar logs: `az containerapp logs show --name "sd2-{env}-sync" --resource-group "rg-sd2-{env}"`
2. Verify `GQL_FABRIC_ENDPOINT` is set: look for the endpoint URL in the logs
3. Verify `GQL_FABRIC_CHANGELOG=1` is set on the main container -- without this,
   the GQL module does not write to the `__sd2:changelog` stream

### Changelog stream growing unbounded

**Symptom:** `XLEN __sd2:changelog` keeps increasing and the sidecar is not
consuming events.

**Cause:** The sidecar may have crashed or failed authentication.

**Fix:**
1. Check sidecar logs for authentication errors
2. Verify `AZURE_CLIENT_SECRET` has not expired (rotates every 90 days --
   see [Secrets Configuration](secrets))
3. Restart the sidecar: `az containerapp revision restart --name "sd2-{env}-sync" --resource-group "rg-sd2-{env}"`

### Authentication failures in REST API calls

**Symptom:** REST API returns `401 Unauthorized` or `403 Forbidden`.

**Cause:** Token expired, wrong scope, or service principal lacks Fabric permissions.

**Fix:**
1. Confirm you are requesting scope `https://api.fabric.microsoft.com/.default`
2. Verify the service principal is a member or admin of the target Fabric workspace
3. Request a fresh token -- Fabric tokens expire after 60 minutes

### Data appears in Fabric but is stale

**Symptom:** Recent mutations are missing from Fabric queries.

**Cause:** Replication lag. The sidecar processes events in configurable batches.

**Fix:**
1. Check `SD2_SYNC_BLOCK_MS` (default 1000 ms) and `SD2_SYNC_BATCH` (default 100
   events per batch) on the sidecar container
2. Wait a few seconds and re-query -- replication is near-real-time but not instant
3. Check sidecar logs for retry messages indicating transient Fabric API errors

## Related

- [FABRIC_DATA_FLOW.md](FABRIC_DATA_FLOW.md) - deeper explanation of the replication pipeline
- [secrets.md](secrets.md) - Fabric and Azure credential sources used by the deployment
