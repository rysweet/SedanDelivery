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
redis-cli -h localhost -p 6379

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

## Use it in your app

Connect with any Redis client. The module exposes these commands:

```
GQL.CREATE-GRAPH <name>                    Create a named graph
GQL.MUTATE  <graph> "<cypher>"             Create/update/delete data
GQL.QUERY   <graph> "<cypher>"             Read data
GQL.EXPLAIN <graph> "<cypher>"             Show the query plan
GQL.DELETE-GRAPH <name>                    Drop a graph
GQL.LIST-GRAPHS                            List all graphs
```

### Python example

```python
import redis

r = redis.Redis(host="your-endpoint.azurecontainerapps.io", port=6379,
                password="your-valkey-password")

r.execute_command("GQL.CREATE-GRAPH", "social")
r.execute_command("GQL.MUTATE", "social",
    "CREATE (a:Person {name: 'Alice'})-[:KNOWS]->(b:Person {name: 'Bob'})")

result = r.execute_command("GQL.QUERY", "social",
    "MATCH (n:Person)-[:KNOWS]->(m) RETURN n.name, m.name")
print(result)  # JSON rows
```

### .NET example (StackExchange.Redis)

```csharp
var redis = ConnectionMultiplexer.Connect("your-endpoint:6379,password=...");
var db = redis.GetDatabase();

db.Execute("GQL.CREATE-GRAPH", "social");
db.Execute("GQL.MUTATE", "social",
    "CREATE (a:Person {name: 'Alice'})-[:KNOWS]->(b:Person {name: 'Bob'})");

var result = db.Execute("GQL.QUERY", "social",
    "MATCH (n:Person) RETURN n.name, n.age");
```

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
- Azure Container Registry (builds the Rust module)
- Container Apps Environment with Azure Files volume (data persists across restarts)
- Valkey 8.1 container with the GQL module loaded and authentication enabled

Cleanup: remove the `deploy` label, or run the **Undeploy** workflow.

## Documentation

- [Tutorial](docs/TUTORIAL.md) — full usage guide with code examples
- [Docs site](https://cloud-ecosystem-security.github.io/SedanDelivery/) — deployment, storage backends, environment variables
- [Research](docs/RESEARCH.md) — security analysis

## Tests

**100% openCypher TCK compliance — 3903/3903 scenarios.**

```bash
bash scripts/validate.sh           # full validation suite (all tiers, ~5 min)
bash scripts/validate.sh --fast    # Tier 1+2 only (fmt + clippy + 128 tests, ~2 min)
bash scripts/validate.sh --tck     # include TCK 3903 scenarios (~3 min extra)
```

| Layer | Count | What it validates |
|-------|-------|------------------|
| Unit tests | 385 | Parser, planner, executor, storage, budget |
| End-to-end + pipeline | 46 | Full parse → plan → execute → result |
| Data integrity | 23 | MERGE atomicity, property round-trips, referential integrity |
| Adversarial | 11 | DoS guards: size limits, depth limits, budget enforcement |
| Stress + scale | 8 | 10 K node load, label scan, budget at scale |
| LDBC SNB | 37 | IC1–IC14 complex reads; all 8 insert + 8 delete operations |
| Module integration | 18 | GQL command pipeline (GQL.QUERY, GQL.MUTATE, etc.) |
| Wire format | 28 | JSON serialisation for all value types |
| openCypher TCK | 3903 | 100% compliance, all 195 feature files |

See [docs/validation.md](docs/validation.md) for the full guide: what each suite proves,
CI tiers, how to re-run from scratch, and performance baseline tracking.

## License

MIT OR Apache-2.0
