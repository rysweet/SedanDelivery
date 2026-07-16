# SedanDelivery

A multi-language graph database engine for Valkey/Redis. Supports openCypher, Apache TinkerPop Gremlin, and GQL (ISO/IEC 39075) — all sharing the same storage layer and executor. MIT/Apache-2.0 licensed.

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

# 4. Deploy manually with the Azure CLI / Bicep templates in infra/
#    (see docs/DEPLOY.md for the full step-by-step guide)
az deployment group create --resource-group "<your-rg>" \
  --template-file infra/main.bicep --parameters @infra/main.parameters.json
```

> **Note:** The automated GitHub Actions deployment workflows (`deploy.yml`,
> `undeploy.yml`, `cleanup-deployments.yml`, GHCR/Pages publishing, and auto
> release) were removed when this repo came under Simard stewardship, which
> focuses on CI, quality, and supply-chain health rather than automated
> deployment. Deploy manually using the Bicep templates in `infra/` — see
> [docs/DEPLOY.md](docs/DEPLOY.md).

## Embed in your Rust app (no server needed)

SedanDelivery's three query engines also ship as ordinary Rust crates so you
can run graph queries entirely in-process — no Redis, no Bolt, no network.
Pick the language that fits your team:

```toml
# Cargo.toml — pick one
sedan-cypher-embedded  = { git = "https://github.com/cloud-ecosystem-security/SedanDelivery" }  # openCypher
sedan-gql-embedded     = { git = "https://github.com/cloud-ecosystem-security/SedanDelivery" }  # GQL (ISO/IEC 39075)
sedan-gremlin-embedded = { git = "https://github.com/cloud-ecosystem-security/SedanDelivery" }  # Apache TinkerPop Gremlin
```

```rust
use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore};

let engine = CypherEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();
engine.execute(&store, "g", "CREATE (:Person {name: 'Alice'})").unwrap();
let r = engine.execute(&store, "g", "MATCH (p:Person) RETURN p.name").unwrap();
```

Try the canonical three-engine demo:

```bash
cargo run -p embedded_quickstart
```

See **[docs/EMBEDDED_GUIDE.md](docs/EMBEDDED_GUIDE.md)** for the full guide:
quick starts for each engine, storage backend choices (in-memory / WAL /
Redis / Fabric), `ExecutionBudget` configuration, parameter binding,
concurrency patterns (`Arc<Engine>` + `Arc<Store>`), performance tips, and a
side-by-side cypher ↔ gql ↔ gremlin migration table.

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

Browser WebSocket clients must set `BOLT_WS_ALLOWED_ORIGINS` to an explicit allowlist; when it is set, SedanDelivery requires a matching `Origin` header on the WebSocket upgrade.

**Validated with real-world apps** — see the [Neo4j Compatibility Gallery](https://turbo-adventure-3eloook.pages.github.io/neo4j-compat-gallery) for 4 security-focused open-source apps running against SedanDelivery with zero code modifications.

See [docs/specs/sedandelivery-bolt-spec.md](docs/specs/sedandelivery-bolt-spec.md) for the full Bolt protocol specification.

## Connect via Gremlin WebSocket

Apache TinkerPop Gremlin drivers connect to the WebSocket server on port 8182. Two server binaries are available:

```bash
# Bytecode protocol — for standard TinkerPop drivers (recommended)
cargo run -p sedandelivery-gremlin
# Listens on 0.0.0.0:8182 by default

# Text eval protocol — for Gremlin text queries (eval op)
cargo run -p sedandelivery-gremlin-server
# Also listens on 0.0.0.0:8182 by default
```

- **sedandelivery-gremlin** — Accepts Gremlin bytecode (TinkerPop driver protocol). Decodes GraphSON v3 bytecode messages, compiles steps via `gremlin-core`, and executes through the shared cypher-core pipeline.
- **sedandelivery-gremlin-server** — Accepts Gremlin text queries (`eval` op). Parses traversal strings, compiles to operator trees, and executes.

Both servers use the same `GraphStore` backend as the Bolt and REST servers. Results are returned in GraphSON v3 format.

**Authentication**: Set `GREMLIN_AUTH_TOKEN` to enable bearer token auth on `sedandelivery-gremlin-server`. Clients authenticate via `Authorization: Bearer <token>` header or a `bearer` field in the first message. See [docs/auth.md](docs/auth.md#gremlin-websocket-auth) for details.

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

Deployment is performed manually via the Azure Bicep templates in `infra/`
(the automated GitHub Actions deployment workflows have been removed — see the
note in [Deploy to Azure](#deploy-to-azure)).

Each environment gets its own resource group (`rg-sd2-{name}`) with:
- Azure Container Registry (builds Docker images)
- Valkey 8.1 container with GQL module (Redis :6379)
- Standalone Bolt server container (Bolt :7687, scalable)
- Standalone Gremlin server container (WebSocket :8182, scalable)
- Azure Files persistent volume for RDB/AOF
- Optional: Azure Cache for Redis (managed HA with TLS and automatic failover)
- Optional: Redis Cluster mode (horizontal data sharding via `GQL_REDIS_CLUSTER_NODES`)
- Optional: Read-replica routing (offload reads via `GQL_REDIS_REPLICA_URL`, round-robin with eventual consistency)
- Optional: Fabric sync sidecar (replication to Microsoft Fabric)

**Full deployment guide**: [docs/DEPLOY.md](docs/DEPLOY.md) — prerequisites, step-by-step setup, scaling, HA configuration, troubleshooting.

Cleanup: delete the environment's resource group with `az group delete`.

## Fabric Graph Replication

Every deployment automatically replicates graph data to [Microsoft Fabric](https://learn.microsoft.com/fabric/). No extra configuration is needed -- `ENABLE_FABRIC` is `true` by default.

**How it works:**

1. The GQL module writes every mutation to a Valkey stream (`__sd2:changelog`)
2. The `sd2-{env}-sync` Container App, built from the `sedandelivery-fabric-sync` image, reads that stream
3. The sidecar refreshes the Fabric GraphModel through the Fabric REST API

This gives you low-latency reads from the co-located Valkey container and a durable, queryable graph in Fabric for analytics, cross-service joins, and the Fabric portal query editor.

**View your data in Fabric:**

1. Open [app.fabric.microsoft.com](https://app.fabric.microsoft.com)
2. Find the **SD2-{env}** workspace (e.g., `SD2-mybranch`)
3. Open the **sedangraph** GraphModel
4. Browse nodes and relationships, or run openCypher queries in the query editor

See [docs/FABRIC_TUTORIAL.md](docs/FABRIC_TUTORIAL.md) for the complete walkthrough: writing data, querying in Fabric, REST API access, and monitoring replication.

## REST API (`sd2-api`)

A standalone HTTP API for graph operations (CRUD, Cypher queries, export/import):

```bash
cargo run -p sd2-api   # listens on 0.0.0.0:3001
```

**Authentication**: Set `GQL_API_ENTRA_TENANT_ID` and `GQL_API_ENTRA_CLIENT_ID` to enable Entra ID JWT auth. All requests (except `/health`) require a valid `Authorization: Bearer <token>` header. See [docs/auth.md](docs/auth.md) for setup.

## Security Considerations

### Redis Cluster mode requires TLS

When `GQL_REDIS_CLUSTER_NODES` is set, all seed URLs **must** use the
`rediss://` scheme (TLS). Plaintext `redis://` seed URLs are only permitted
for loopback addresses in local development.

```bash
# Production cluster (TLS required)
GQL_REDIS_CLUSTER_NODES="rediss://node1:6379,rediss://node2:6379,rediss://node3:6379"

# Local development only (loopback)
GQL_REDIS_CLUSTER_NODES="redis://127.0.0.1:7001,redis://127.0.0.1:7002,redis://127.0.0.1:7003"
```

Inter-node cluster traffic carries authentication credentials and graph data.
Plaintext exposes both to network observers. See
[docs/cluster-mode.md](docs/cluster-mode.md) for the full cluster setup guide.

## Documentation

- [Authentication](docs/auth.md) — Configure auth for the REST API (Entra ID JWT), Bolt (password), and Gremlin WebSocket (bearer token)
- [Neo4j Compatibility Gallery](https://turbo-adventure-3eloook.pages.github.io/neo4j-compat-gallery) — 4 security apps validated against SedanDelivery's Bolt protocol
- [Tutorial](docs/TUTORIAL.md) — full usage guide with code examples
- [Embedded Library Usage Guide](docs/EMBEDDED_GUIDE.md) — comprehensive guide for the in-process crates (`sedan-cypher-embedded`, `sedan-gql-embedded`, `sedan-gremlin-embedded`), with runnable examples under each crate's `examples/` directory
- [Performance Benchmarking](docs/PERFORMANCE.md) — criterion bench suite for the embedded engine crates with run/interpret/baseline guidance
- [Product Surface Reference](docs/reference/product-surfaces.md) — current runtime/distribution surfaces and the live comparison baselines for embedded engines
- [Embedded Engines Reference](docs/reference/embedded-engines.md) — current public API and wiring for the in-process crates
- [Embed SedanDelivery In-Process](docs/tutorials/embed-sedandelivery.md) — current embedded quick start for Cypher, GQL, and Gremlin
- [Pick Your Embedded Engine](docs/howto/pick-embedded-engine.md) — short decision guide and `Cargo.toml` cheat-sheet
- [Bolt Protocol Spec](docs/specs/sedandelivery-bolt-spec.md) — Neo4j Bolt v4.4 server implementation details
- [Docs site](https://turbo-adventure-3eloook.pages.github.io/) — deployment, storage backends, environment variables
- [Architecture Overview](docs/architecture.md) — crate map, three query engines, request flows, query pipeline, storage layer

## Tests

**100% openCypher TCK compliance — 3903/3903 scenarios.**

```bash
bash scripts/validate.sh           # full validation suite, including sedan-web lint/test/build
bash scripts/validate.sh --fast    # PR-path validation, including sedan-web + Rust fast tiers
bash scripts/validate.sh --tck     # include TCK 3903 scenarios (~3 min extra)
```

| Layer | Count | What it validates |
|-------|-------|------------------|
| cypher-core unit | 678 | Parser, planner, executor, storage, budget, config |
| sedandelivery-bolt unit | 399 | PackStream codec, message framing, Bolt state machine, bridge layer |
| sd2-api unit | 115 | API handler pure functions |
| sd2-redis-store unit | 59 | Redis storage layer logic |
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
| Gremlin TCK | 1672 | TinkerPop-equivalent scenarios (modern, crew, grateful dead graphs) |
| openCypher TCK | 3903 | 100% compliance, all 195 feature files |
| Bolt driver compat | 3 drivers | neo4j (Python), Neo4j.Driver (.NET), neo4rs (Rust) |

See [docs/validation.md](docs/validation.md) for the full guide: what each suite proves,
CI tiers, how to re-run from scratch, and performance baseline tracking.
## License

MIT OR Apache-2.0
