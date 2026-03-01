---
layout: default
title: SedanDelivery Documentation
---

# SedanDelivery Documentation

A Rust GQL/Cypher query engine — a clean-room, MIT/Apache-2.0-licensed replacement for RedisGraph/FalkorDB.

## Guides

- [Tutorial](TUTORIAL) — Getting started with SedanDelivery
- [Fabric Graph Tutorial](FABRIC_TUTORIAL) — Deploy, write data, query in Fabric portal, REST API, and monitoring
- [Validation Guide](validation) — Full test suite: what was validated, how to re-run, CI tiers, benchmark baselines
- [Secrets Configuration](secrets) — Repository secrets inventory, creation, rotation, and monitoring
- [Benchmarks](benchmarks) — How to run and interpret performance benchmarks
- [Research Notes](RESEARCH) — Security analysis and design rationale

## Neo4j Compatibility

- [**Neo4j Compatibility Gallery**](neo4j-compat-gallery) — 3 security apps validated against SedanDelivery's Bolt v4.4 protocol with screenshots (neovis.js threat visualization, GraphKer CVE explorer, Network Security attack surface analyzer)

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
| `BOLT_BIND_ADDR` | `0.0.0.0:7687` | Bolt TCP listen address |
| `BOLT_PASSWORD` | — | Optional password for Bolt connections |
| `BOLT_ENABLED` | `true` | Enable/disable Bolt server in container |
| `GQL_REDIS_URL` | `redis://127.0.0.1:6379` | Redis connection URL for RedisGraphStore |
| `GQL_FABRIC_ENDPOINT` | — | Fabric Graph Database REST API URL |
| `GQL_FABRIC_TENANT_ID` | — | Azure AD tenant for Fabric auth |
| `GQL_FABRIC_CLIENT_ID` | — | Service principal app ID for Fabric |
| `GQL_FABRIC_CLIENT_SECRET` | — | Service principal secret for Fabric |
| `VALKEY_PASSWORD` | — | Password for Valkey authentication (injected at runtime) |

## Bolt Protocol (Recommended)

SedanDelivery includes a built-in Neo4j Bolt v4.4 server (`sedandelivery-bolt`). Any standard Neo4j driver connects directly — no custom SDK needed.

```bash
# Build and run the Bolt server
cargo run -p sedandelivery-bolt
# Listens on 0.0.0.0:7687 (configure via BOLT_BIND_ADDR env var)
# Optional auth via BOLT_PASSWORD env var
```

### Python (neo4j driver)
```bash
pip install neo4j
```
```python
from neo4j import GraphDatabase

driver = GraphDatabase.driver("bolt://localhost:7687", auth=None)
with driver.session() as session:
    session.run("CREATE (:Person {name: 'Alice', age: 30})")
    result = session.run("MATCH (p:Person) RETURN p.name AS name, p.age AS age")
    for record in result:
        print(record["name"], record["age"])
driver.close()
```

### .NET (Neo4j.Driver)
```bash
dotnet add package Neo4j.Driver
```
```csharp
using Neo4j.Driver;

var driver = GraphDatabase.Driver("bolt://localhost:7687", AuthTokens.None);
await using var session = driver.AsyncSession();
await session.RunAsync("CREATE (:Person {name: 'Alice', age: 30})");
var result = await session.RunAsync("MATCH (p:Person) RETURN p.name AS name, p.age AS age");
await foreach (var record in result)
    Console.WriteLine($"{record["name"].As<string>()} {record["age"].As<long>()}");
```

### Rust (neo4rs)
```toml
[dependencies]
neo4rs = "0.8"
tokio = { version = "1", features = ["full"] }
```
```rust
use neo4rs::*;

let graph = Graph::connect(
    ConfigBuilder::default()
        .uri("bolt://localhost:7687")
        .user("").password("").build()?
).await?;
graph.run(query("CREATE (:Person {name: 'Alice', age: 30})")).await?;
let mut result = graph.execute(
    query("MATCH (p:Person) RETURN p.name AS name, p.age AS age")
).await?;
while let Some(row) = result.next().await? {
    println!("{} {}", row.get::<String>("name")?, row.get::<i64>("age")?);
}
```

### Java, Go, JavaScript
Any Neo4j driver that supports Bolt 4.2-4.4 works. See the [Neo4j Driver Manual](https://neo4j.com/docs/create-applications/) for your language.

### Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `BOLT_BIND_ADDR` | `0.0.0.0:7687` | Bolt TCP listen address |
| `BOLT_PASSWORD` | (none) | Optional password for Bolt auth |
| `BOLT_ENABLED` | `true` | Enable/disable Bolt server in container |

See [Bolt Protocol Spec](specs/sedandelivery-bolt-spec) for implementation details.

## Custom Client SDKs (Redis protocol)

For direct Redis protocol access, SedanDelivery also provides custom client libraries:

### Python
```bash
pip install sedandelivery   # or: pip install -e sdk/python
```
```python
from sedandelivery import SedanDB
db = SedanDB(host="localhost", password="secret")
g = db.select_graph("social")
g.create()
g.mutate("CREATE (:Person {name: $name})", name="Alice")
for r in g.query("MATCH (n) RETURN n.name"):
    print(r["name"])
```

### .NET
```bash
dotnet add package SedanDelivery.Client
```
```csharp
using var db = new SedanDB("localhost:6379,password=secret");
var g = db.SelectGraph("social");
g.Create();
g.Mutate("CREATE (:Person {name: $name})", new { name = "Alice" });
foreach (var r in g.Query("MATCH (n) RETURN n.name"))
    Console.WriteLine(r.Get<string>("name"));
```

### Rust
```toml
[dependencies]
sedan-client = { path = "sdk/rust/sedan-client" }
```
```rust
let db = SedanDB::new("redis://localhost:6379")?;
let mut g = db.select_graph("social")?;
g.create()?;
g.mutate("CREATE (:Person {name: 'Alice'})")?;
for record in g.query("MATCH (n) RETURN n.name")? {
    println!("{}", record.get_str("name").unwrap());
}
```

See [sdk/README.md](../sdk/README.md) for the full Redis-protocol SDK API reference.

## Fabric Graph Replication

`ENABLE_FABRIC` is **`true` by default** in `deploy.yml`. Every deployment provisions a
[Microsoft Fabric Graph Database](https://learn.microsoft.com/fabric/) workspace alongside
the Container App and deploys a `sedandelivery-fabric-sync` sidecar that continuously
replicates graph mutations to Fabric.

**Architecture:**

```
┌──────────────────────────────────────────────────────────────┐
│  Azure Container Apps Environment                            │
│                                                              │
│  ┌─────────────────────┐    ┌──────────────────────────┐     │
│  │  sd2-{env}-redis    │    │  sd2-{env}-fabric-sync   │     │
│  │  (Valkey + GQL mod) │    │  (sync sidecar)          │     │
│  │                     │    │                          │     │
│  │  Bolt :7687         │    │  Reads __sd2:changelog   │     │
│  │  Redis :6379        │    │  stream from Valkey,     │     │
│  │                     │    │  writes to Fabric REST   │     │
│  │  GQL_FABRIC_        │    │  API                     │     │
│  │  CHANGELOG=1        │    │                          │     │
│  └─────────┬───────────┘    └────────────┬─────────────┘     │
│            │ __sd2:changelog stream       │                   │
│            └─────────────────────────────→│                   │
└──────────────────────────────────────────┼───────────────────┘
                                           │ HTTPS
                                           ▼
                              ┌─────────────────────────┐
                              │  Microsoft Fabric        │
                              │  Workspace:              │
                              │  SedanDelivery-{env}     │
                              │  Graph DB: sedan-graph   │
                              └─────────────────────────┘
```

**What you get:**

- **Low-latency reads** from the co-located Valkey container
- **Durable, queryable graph** in Fabric for analytics and cross-service joins
- **Automatic provisioning** via `scripts/provision-fabric.sh`
- **Portal query editor** -- run openCypher queries directly in Fabric

The Fabric endpoint and credentials are injected as Container App environment variables
(`GQL_FABRIC_ENDPOINT`, `GQL_FABRIC_TENANT_ID`, `GQL_FABRIC_CLIENT_ID`) with the client
secret stored as a Container App secret (`secretref:fabric-client-secret`).

See the [Fabric Graph Tutorial](FABRIC_TUTORIAL) for a complete walkthrough: deploying,
writing data, viewing in the Fabric portal, REST API access, and monitoring replication.

## Source Code

- [GitHub Repository](https://github.com/cloud-ecosystem-security/SedanDelivery)

## Internal

- [Design decisions](https://github.com/cloud-ecosystem-security/SedanDelivery/tree/main/docs) (ADRs)
