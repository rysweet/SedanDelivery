# SedanDelivery

A graph database engine for Valkey/Redis. MIT/Apache-2.0 licensed.

## Getting started

**Step 1** — Clone and run the demo (30 seconds):

```bash
git clone https://github.com/cloud-ecosystem-security/SedanDelivery.git
cd SedanDelivery
cargo run -p gql-cli
```

You'll see a graph created with people and companies, queries executed, and results printed. No Redis, no cloud account, no config.

**Step 2** — Create your first graph (2 minutes):

Once the demo finishes, the engine is ready. Here's what it looks like from a Redis client:

```bash
# Connect to a running SedanDelivery instance (local or deployed)
REDISCLI_AUTH="${VALKEY_PASSWORD:?set VALKEY_PASSWORD first}" redis-cli -h localhost -p 6379  # local compose defaults require auth

# Create a graph and add data
GQL.CREATE-GRAPH social
GQL.MUTATE social "CREATE (a:Person {name: 'Alice', age: 30})-[:KNOWS]->(b:Person {name: 'Bob', age: 25})"
GQL.MUTATE social "CREATE (c:Company {name: 'Acme'})"
GQL.MUTATE social "MATCH (a:Person {name: 'Alice'}), (c:Company {name: 'Acme'}) CREATE (a)-[:WORKS_AT]->(c)"

# Query it
GQL.QUERY social "MATCH (p:Person)-[:KNOWS]->(f) RETURN p.name, f.name"
GQL.QUERY social "MATCH (p:Person)-[:WORKS_AT]->(c:Company) RETURN p.name, c.name"
GQL.QUERY social "MATCH (p:Person) RETURN avg(p.age) AS avg_age, count(p) AS total"
```

**Step 3** — Deploy to Azure (5 minutes):

## Deploy to Azure

Deploy your own SedanDelivery instance in under 5 minutes:

```bash
# 1. Fork and clone the repo
gh repo fork cloud-ecosystem-security/SedanDelivery --clone

# 2. Set up Azure credentials (one-time)
az login
SUBSCRIPTION_ID=$(az account show --query id -o tsv)
SP=$(az ad sp create-for-rbac --name sedan-deploy --role Contributor \
  --scopes "subscriptions/${SUBSCRIPTION_ID}" --output json)

# 3. Configure GitHub secrets
gh secret set AZURE_CLIENT_ID     --body "$(echo $SP | jq -r .appId)"
gh secret set AZURE_CLIENT_SECRET --body "$(echo $SP | jq -r .password)"
gh secret set AZURE_TENANT_ID     --body "$(echo $SP | jq -r .tenant)"
gh secret set AZURE_SUBSCRIPTION_ID --body "${SUBSCRIPTION_ID}"

# 4. Deploy (triggers the workflow manually)
gh workflow run deploy.yml -f environment=branch

# 5. Or: create a PR and add the 'deploy' label
gh pr create --title "my feature" --body "test deploy"
gh pr edit --add-label deploy
```

A bot comment will post your Redis endpoint when the deployment completes (~8 minutes).

## Connect via Bolt protocol (recommended)

Standard Neo4j drivers connect directly — no custom SDK needed:

```python
from neo4j import GraphDatabase

driver = GraphDatabase.driver("bolt://localhost:7687", auth=None)
with driver.session() as session:
    session.run("CREATE (:Person {name: 'Alice', age: 30})")
    result = session.run("MATCH (p:Person) RETURN p.name, p.age")
    for record in result:
        print(record["p.name"], record["p.age"])
driver.close()
```

Works with any Neo4j driver: Python, .NET, Java, Go, Rust, JavaScript.

```bash
# Build and run sedandelivery-bolt
cargo run -p sedandelivery-bolt
# Listens on 0.0.0.0:7687 by default (configure via BOLT_BIND_ADDR env var)
```

**Validated with real-world apps** — see the [Neo4j Compatibility Gallery](https://turbo-adventure-3eloook.pages.github.io/neo4j-compat-gallery) for 4 security-focused open-source apps running against SedanDelivery with zero code modifications.

See [docs/specs/sedandelivery-bolt-spec.md](docs/specs/sedandelivery-bolt-spec.md) for the full Bolt protocol specification.

## Use it via Redis protocol

Connect with any Redis client. The module exposes these commands:

```
GQL.CREATE-GRAPH <name>                    Create a named graph
GQL.MUTATE  <graph> "<cypher>"             Create/update/delete data
GQL.QUERY   <graph> "<cypher>"             Read data
GQL.EXPLAIN <graph> "<cypher>"             Show the query plan
GQL.DELETE-GRAPH <name>                    Drop a graph
GQL.LIST-GRAPHS                            List all graphs
```

### Python (`pip install sedandelivery`)

```python
from sedandelivery import SedanDB

db = SedanDB(host="your-endpoint.azurecontainerapps.io", port=6379,
             password="your-valkey-password")
g = db.select_graph("social")
g.create()

# Parameterized mutations
g.mutate("CREATE (a:Person {name: $name, age: $age})-[:KNOWS]->(b:Person {name: $friend})",
         name="Alice", age=30, friend="Bob")

# Query with structured results
for record in g.query("MATCH (p:Person) WHERE p.age > $min RETURN p.name, p.age", min=20):
    print(record["name"], record["age"])   # dict-like access

# Mutation stats
result = g.mutate("CREATE (:Company {name: $co})", co="Acme")
print(result.stats.nodes_created)  # 1
```

### .NET (`dotnet add package SedanDelivery.Client`)

```csharp
using SedanDelivery.Client;

using var db = new SedanDB("your-endpoint:6379,password=...");
var g = db.SelectGraph("social");
g.Create();

// Parameterized mutations
g.Mutate("CREATE (a:Person {name: $name})", new { name = "Alice" });

// Query with typed results
var result = g.Query("MATCH (p:Person) RETURN p.name, p.age");
foreach (var record in result) {
    Console.WriteLine($"{record.Get<string>("name")} is {record.Get<int>("age")}");
}

// Async support
var count = await g.QueryAsync("MATCH (n) RETURN count(n) AS total");
```

### Rust (`sedan-client`)

```rust
use sedan_client::SedanDB;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = SedanDB::new("redis://your-endpoint:6379")?;
    let mut g = db.select_graph("social")?;
    g.create()?;

    // Create data
    g.mutate("CREATE (a:Person {name: 'Alice', age: 30})-[:KNOWS]->(b:Person {name: 'Bob', age: 25})")?;

    // Query with structured results
    for record in g.query("MATCH (p:Person) WHERE p.age > 20 RETURN p.name, p.age")? {
        println!("{} is {}", record.get_str("p.name").unwrap(), record.get_i64("p.age").unwrap());
    }

    Ok(())
}
```

See [sdk/README.md](sdk/README.md) for the full API reference covering all three SDKs.

### Cypher query examples

```cypher
-- Create nodes with labels and properties
CREATE (n:Person {name: 'Alice', age: 30})
CREATE (n:Company {name: 'Acme', founded: 2020})

-- Create relationships
MATCH (a:Person {name: 'Alice'}), (c:Company {name: 'Acme'})
CREATE (a)-[:WORKS_AT {since: 2022}]->(c)

-- Query with patterns
MATCH (p:Person)-[:WORKS_AT]->(c:Company)
WHERE p.age > 25
RETURN p.name, c.name

-- Aggregation
MATCH (p:Person)
RETURN avg(p.age) AS avg_age, count(p) AS total

-- Variable-length paths
MATCH (a:Person)-[:KNOWS*1..3]->(b:Person)
RETURN a.name, b.name
```

## Deployment details

| Trigger | Environment | What happens |
|---------|-------------|-------------|
| PR + `deploy` label | Isolated per-branch | Provisions ACR + Container Apps + persistent storage |
| Push to `main` | Production | Updates the production deployment |
| `workflow_dispatch` | Manual | Deploy any branch on demand |

Each environment gets its own resource group (`rg-sd2-{name}`) with:
- Azure Container Registry (builds Docker images)
- Valkey 8.1 container with GQL module (Redis :6379)
- Standalone Bolt server container (Bolt :7687, scalable)
- Azure Files persistent volume for RDB/AOF
- Optional: Azure Cache for Redis (managed HA with TLS and automatic failover)
- Optional: Redis Cluster mode (horizontal data sharding via `GQL_REDIS_CLUSTER_NODES`)
- Optional: Read-replica routing (offload reads via `GQL_REDIS_REPLICA_URL`, round-robin with eventual consistency)
- Optional: Fabric sync sidecar (replication to Microsoft Fabric)

**Full deployment guide**: [docs/DEPLOY.md](docs/DEPLOY.md) — prerequisites, step-by-step setup, scaling, HA configuration, troubleshooting.

Cleanup: remove the `deploy` label, or run the **Undeploy** workflow.

## Fabric Graph Replication

Every deployment automatically replicates graph data to [Microsoft Fabric Graph Database](https://learn.microsoft.com/fabric/). No extra configuration is needed -- `ENABLE_FABRIC` is `true` by default.

**How it works:**

1. The GQL module writes every mutation to a Valkey stream (`__sd2:changelog`)
2. A `sedandelivery-fabric-sync` sidecar container reads that stream
3. The sidecar replays each change against the Fabric Graph Database REST API

This gives you low-latency reads from the co-located Valkey container and a durable, queryable graph in Fabric for analytics, cross-service joins, and the Fabric portal query editor.

**View your data in Fabric:**

1. Open [app.fabric.microsoft.com](https://app.fabric.microsoft.com)
2. Find the **SedanDelivery-{env}** workspace (e.g., `SedanDelivery-mybranch`)
3. Open the **sedan-graph** Graph Database
4. Browse nodes and relationships, or run openCypher queries in the query editor

See [docs/FABRIC_TUTORIAL.md](docs/FABRIC_TUTORIAL.md) for the complete walkthrough: writing data, querying in Fabric, REST API access, and monitoring replication.

## REST API (`gql-api`)

A standalone HTTP API for graph operations (CRUD, Cypher queries, export/import):

```bash
cargo run -p gql-api   # listens on 0.0.0.0:3001
```

**Authentication**: Set `GQL_API_ENTRA_TENANT_ID` and `GQL_API_ENTRA_CLIENT_ID` to enable Entra ID JWT auth. All requests (except `/health`) require a valid `Authorization: Bearer <token>` header. See [docs/auth.md](docs/auth.md) for setup.

## Documentation

- [Authentication (Entra ID)](docs/auth.md) — Configure JWT auth for the REST API
- [Neo4j Compatibility Gallery](https://turbo-adventure-3eloook.pages.github.io/neo4j-compat-gallery) — 4 security apps validated against SedanDelivery's Bolt protocol
- [Tutorial](docs/TUTORIAL.md) — full usage guide with code examples
- [Bolt Protocol Spec](docs/specs/sedandelivery-bolt-spec.md) — Neo4j Bolt v4.4 server implementation details
- [Docs site](https://turbo-adventure-3eloook.pages.github.io/) — deployment, storage backends, environment variables
- [Architecture Overview](docs/architecture.md) — crate map, request flows, query pipeline, storage layer

## Tests

**100% openCypher TCK compliance — 3903/3903 scenarios.**

```bash
bash scripts/validate.sh           # full validation suite (all tiers, ~5 min)
bash scripts/validate.sh --fast    # Tier 1+2 only (fmt + clippy + 80 integration tests, ~2 min)
bash scripts/validate.sh --tck     # include TCK 3903 scenarios (~3 min extra)
```

| Layer | Count | What it validates |
|-------|-------|------------------|
| gql-core unit | 678 | Parser, planner, executor, storage, budget, config |
| sedandelivery-bolt unit | 399 | PackStream codec, message framing, Bolt state machine, bridge layer |
| gql-api unit | 115 | API handler pure functions |
| gql-redis-store unit | 59 | Redis storage layer logic |
| gql-fabric-store unit | 45 | Fabric storage layer logic |
| sedandelivery-bolt integration | 178 | Python neo4j driver: CRUD, transactions, paths, concurrency |
| End-to-end + pipeline | 42 | Full parse → plan → execute → result |
| Exists / interactive | 6 | EXISTS subqueries and pattern predicates |
| Data integrity | 23 | MERGE atomicity, property round-trips, referential integrity |
| Adversarial | 11 | DoS guards: size limits, depth limits, budget enforcement |
| Stress + scale | 8 | 10 K node load, label scan, budget at scale |
| LDBC SNB | 37 | IC1–IC14 complex reads; all 8 insert + 8 delete operations |
| Module integration | 18 | GQL command pipeline (GQL.QUERY, GQL.MUTATE, etc.) |
| Wire format | 28 | JSON serialisation for all value types |
| openCypher TCK | 3903 | 100% compliance, all 195 feature files |
| Bolt driver compat | 3 drivers | neo4j (Python), Neo4j.Driver (.NET), neo4rs (Rust) |

See [docs/validation.md](docs/validation.md) for the full guide: what each suite proves,
CI tiers, how to re-run from scratch, and performance baseline tracking.
## License

MIT OR Apache-2.0
