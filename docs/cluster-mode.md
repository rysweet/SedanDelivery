---
layout: default
title: Redis Cluster Mode
---

# Redis Cluster Mode

How to run SedanDelivery against Redis Cluster, how to validate the public Bolt
path, and what guarantees cluster-safety after `#745`.

## Contents

- [Prerequisites](#prerequisites)
- [Quick Start (local cluster)](#quick-start-local-cluster)
- [Configuration](#configuration)
- [Connection Pool Tuning](#connection-pool-tuning)
- [Validating Cluster Behaviour](#validating-cluster-behaviour)
- [Known Limitations](#known-limitations)
- [Troubleshooting](#troubleshooting)

---

## Prerequisites

- Docker with Compose v2 for local cluster bring-up
- `VALKEY_PASSWORD` set in your environment
- Python `neo4j` driver if you want to run the outside-in smoke scripts

---

## Quick Start (local cluster)

The repository ships `docker-compose.cluster.yml`, which starts:

- 3 Redis Cluster masters
- 3 Redis Cluster replicas
- a `cluster-init` container that assigns slots
- `sedandelivery-bolt` wired to `GQL_REDIS_CLUSTER_NODES`

Bring it up:

```bash
export VALKEY_PASSWORD='change-me-for-local-dev'
export BOLT_PASSWORD="$VALKEY_PASSWORD"
docker compose -f docker-compose.cluster.yml up -d --build
```

The deployment is ready when:

- `cluster-init` exits successfully
- `http://localhost:7688/health` returns `200 OK`

### Verify the public Bolt path

Use Bolt session database selection for graph-scoped queries:

```python
import os
from neo4j import GraphDatabase

graph = "social"
driver = GraphDatabase.driver(
    "bolt://localhost:7687",
    auth=("neo4j", os.environ["BOLT_PASSWORD"]),
)

with driver.session() as session:
    session.run(f"CREATE GRAPH {graph}").consume()

with driver.session(database=graph) as session:
    session.run("CREATE (:Person {name: $name})", name="Alice").consume()
    session.run("CREATE (:Person {name: $name})", name="Bob").consume()
    count = session.run(
        "MATCH (p:Person) RETURN count(p) AS c"
    ).single(strict=True)["c"]
    print(count)  # -> 2

with driver.session() as session:
    session.run(f"DROP GRAPH {graph}").consume()

driver.close()
```

That `session(database=graph)` contract matters in cluster mode because it
exercises the same planner and execution path the real server uses.

Tear the environment down:

```bash
docker compose -f docker-compose.cluster.yml down -v
```

---

## Configuration

### Selecting cluster mode

Set `GQL_REDIS_CLUSTER_NODES` to a comma-separated list of Redis Cluster seed
URLs:

```bash
GQL_REDIS_CLUSTER_NODES="rediss://node1:6379,rediss://node2:6379,rediss://node3:6379"
```

SedanDelivery connects to those seeds and discovers the rest of the topology.
You do not need to list every shard member manually.

When `GQL_REDIS_CLUSTER_NODES` is set, it takes priority over `GQL_REDIS_URL`
and `REDIS_URL`.

### TLS

Use `rediss://` whenever the cluster crosses a network boundary. Plaintext
`redis://` is only appropriate for loopback or trusted private development
networks.

### Full variable reference

| Variable | Default | Description |
|----------|---------|-------------|
| `GQL_REDIS_CLUSTER_NODES` | — | Comma-separated cluster seed URLs; enables cluster mode |
| `GQL_REDIS_POOL_SIZE` | `64` | Max connections per pool |
| `GQL_REDIS_POOL_TIMEOUT_SECS` | `5` | Pool checkout timeout (seconds) |
| `GQL_REDIS_IDLE_TIMEOUT_SECS` | `300` | Idle connection reap timeout (seconds) |
| `GQL_REDIS_MIN_IDLE` | `1` | Minimum warm connections kept in pool |
| `GQL_REDIS_KEY_PREFIX` | — | Key prefix for multi-tenant isolation |
| `GQL_REDIS_STATS_INTERVAL_SECS` | `15` | Pool metrics interval (seconds) |

### Hash-tag safety contract

Every per-graph Redis key uses the graph name as a hash tag:

```text
G:{graph}:...
```

That is what keeps graph-local pipelines slot-local in Redis Cluster. Graph
names containing `{` or `}` are rejected because they would break the slot
contract and can cause cross-graph collisions.

See [Redis Key Schema](redis-key-schema) for the full contract.

---

## Connection Pool Tuning

Each Bolt session can hold a pooled Redis connection while a query is running.
Size the pool to match expected concurrent Bolt sessions:

```bash
# 4 Bolt replicas × 16 sessions each = 64 pooled connections
GQL_REDIS_POOL_SIZE=64

# Slower networks or larger batches may need more checkout time
GQL_REDIS_POOL_TIMEOUT_SECS=10
```

Pool utilization metrics:

```text
redis_pool_size
redis_pool_idle
redis_pool_active
```

If `redis_pool_active` stays at `redis_pool_size`, increase the pool or add
Bolt replicas.

---

## Validating Cluster Behaviour

`#745` was fixed and validated at three layers. If you touch cluster code, run
the same layers again.

### 1. Crate test suite

```bash
cargo test -p gql-redis-store
```

### 2. Real cluster integration tests

Expose a host-visible Redis Cluster and point `GQL_REDIS_CLUSTER_NODES` at it:

```bash
GQL_REDIS_CLUSTER_NODES="redis://127.0.0.1:7100,redis://127.0.0.1:7101,redis://127.0.0.1:7102" \
  cargo test -p gql-redis-store --test cluster_pipeline -- --ignored --nocapture
```

`tests/cluster_pipeline.rs` covers:

- cluster-aware batch reads
- index backfill
- graph metadata reads
- `DROP INDEX`
- `DELETE GRAPH`

### 3. Public Bolt resharding test

Run the repo’s end-to-end validator:

```bash
VALKEY_PASSWORD='change-me' BOLT_PASSWORD='change-me' \
  ./scripts/test-resharding-resilience.sh
```

The expected healthy result is:

- zero write errors from the live Bolt load generator
- zero `sedandelivery_bolt_backend_errors_total` increase during resharding

---

## Known Limitations

### No transaction atomicity across a slot-local pipeline

SedanDelivery does **not** use `MULTI/EXEC` in cluster mode. A connection drop
mid-pipeline can still leave partial state. The write lock prevents conflicting
writers from interleaving, but it does not turn a batch into a transaction.

### Metadata registries are membership hints

`G:{g}:_labels` and `G:{g}:_reltypes` are used to avoid cluster-unsafe keyspace
scans. They may retain stale members after deletes, but readers filter those
members by checking the real backing-set cardinality before using them.

---

## Troubleshooting

### `This command cannot be safely routed in cluster mode - Client`

After the final `#745` fix, this is a **code regression indicator**.

Likely causes:

- a new call path executed a generic `redis::Pipeline::query()` on a cluster
  connection instead of going through `cluster_pipe()`
- a planner or cleanup path reintroduced keyspace `SCAN`

Do not try to solve this with Redis topology flags. Fix the code path.

### `CROSSSLOT`

After `#745`, a `CROSSSLOT` error almost always means a key without the graph
hash tag was added to a graph-local batch.

Check:

- new key helpers in `keys.rs`
- new batched write paths
- any manual key construction that bypasses the helpers

### Graph-scoped queries behave oddly from drivers

Make sure the driver selects the graph via the Bolt session/database field:

```python
driver.session(database="my-graph")
```

Do not assume inline graph-selection syntax is interchangeable across every
driver entrypoint.

---

## See Also

- [Cluster Write Safety](cluster-write-safety)
- [Redis Key Schema](redis-key-schema)
- [Documentation Index](index)
