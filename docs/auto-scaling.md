# Auto-Scaling Guide

This guide covers everything an operator needs to enable, configure, monitor, and troubleshoot horizontal scaling in SedanDelivery, consolidating information that was previously spread across DEPLOY.md, architecture.md, and FABRIC_DATA_FLOW.md.

## Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Compute Auto-Scaling (Bolt + API)](#compute-auto-scaling-bolt--api)
- [Write Safety Under Multi-Replica](#write-safety-under-multi-replica)
- [Data Layer](#data-layer)
- [Observability](#observability)
- [Security Considerations](#security-considerations)
- [Testing](#testing)
- [Rollback](#rollback)
- [Configuration Reference](#configuration-reference)

## Overview

### What scales horizontally

| Component | Container | Why it scales |
|-----------|-----------|--------------|
| Bolt server | `sd2-main-bolt` | Stateless — all graph data lives in Redis. Each replica holds only ephemeral connection state. |
| REST API | `sd2-main-api` | Stateless HTTP — identical to Bolt; every request is self-contained against the shared Redis backend. |

### What does NOT auto-scale

| Component | Container | Why |
|-----------|-----------|-----|
| Redis / Valkey | `sd2-main-redis` | Single-primary data tier. Horizontal data sharding (Redis Cluster) is supported but requires manual resharding (see [Data Layer](#data-layer)). Azure Cache for Redis provides HA failover, not auto-scale. |
| Fabric sync | `sd2-main-fabric-sync` | Single-instance consumer. Runs one XREADGROUP consumer against the changelog streams. Adding consumers would split the stream, not increase throughput. |

### Why this design

The compute layer (Bolt, API) is the bottleneck for most workloads — parsing, planning, and serializing queries is CPU-bound. Redis is I/O-bound and typically has headroom. Scaling the stateless query layer is safe and cheap; scaling the data tier requires careful coordination (slot migration, replication lag) and is an operational event, not an auto-scaling trigger.

## Architecture

### Load balancing

Azure Container Apps uses **L4 (TCP) load balancing** via Envoy. This means:

- Balancing happens at the **connection level**, not per-request.
- A long-lived Bolt connection stays pinned to one replica for its entire lifetime.
- New connections are distributed across replicas roughly by round-robin.

This is why connection lifecycle parameters matter — without periodic reconnection, clients that connected before a scale-out event will never reach the new replicas.

### Connection lifecycle

```
Client connects
    |
    v
[Handshake timeout: 5s] ─── BOLT_HANDSHAKE_TIMEOUT_MS
    |
    v
[Active session]
    |
    ├── Idle > 300s? ──────── BOLT_IDLE_TIMEOUT_SECS ──> Close
    |
    └── Alive > 1800s? ───── BOLT_MAX_CONNECTION_LIFETIME_SECS ──> Close
```

| Parameter | Default | Purpose |
|-----------|---------|---------|
| `BOLT_HANDSHAKE_TIMEOUT_MS` | `5000` | Reclaim connection slots from clients that connect but never complete the handshake (port scanners, health probes). |
| `BOLT_IDLE_TIMEOUT_SECS` | `300` | Free slots from abandoned connections. The 5-minute default ensures that after a scale-out, idle clients reconnect and get distributed across new replicas within minutes. |
| `BOLT_MAX_CONNECTION_LIFETIME_SECS` | `1800` | Hard wall-clock cap. Forces even active connections to reconnect every 30 minutes. This is the primary mechanism for rebalancing load across replicas after scale events. |
| TCP keepalive | `120s` | Bolt sends TCP keepalives every 120 seconds to detect dead peers. This must be shorter than the Azure Load Balancer idle timeout (4 minutes / 240 seconds) to prevent the LB from silently dropping the connection. |

**Why these values matter for Azure LB:** Azure's L4 load balancer drops idle TCP connections after 4 minutes. If the Bolt server does not send keepalives within that window, the client sees a silent connection reset. The 120-second keepalive interval provides a 2x safety margin.

## Compute Auto-Scaling (Bolt + API)

### CPU-based scaling (primary trigger)

Container Apps has built-in CPU-based auto-scaling. This is the simplest path:

```bash
# Scale Bolt to 1-10 replicas, triggered by CPU
az containerapp update \
  --name sd2-main-bolt \
  --resource-group rg-sd2-main \
  --min-replicas 1 \
  --max-replicas 10 \
  --scale-rule-name cpu-scaling \
  --scale-rule-type cpu \
  --scale-rule-metadata type=Utilization value=70
```

For the REST API:

```bash
az containerapp update \
  --name sd2-main-api \
  --resource-group rg-sd2-main \
  --min-replicas 1 \
  --max-replicas 3
```

### KEDA Prometheus custom metrics (advanced)

When KEDA 2.13+ is installed and Azure Monitor Managed Prometheus is configured, apply the provided ScaledObject manifests for metric-driven scaling:

```bash
kubectl apply -f infra/keda-scaledobjects.yaml
```

This configures:

| Target | Metric | Threshold | Replicas |
|--------|--------|-----------|----------|
| Bolt server | `sedandelivery_bolt_connections_active` (avg) | 100 per replica | 1-10 |
| Bolt server | `sedandelivery_bolt_query_duration_seconds` (p95) | 500ms | 1-10 |
| REST API | `gql_api_http_concurrent_requests` (avg) | 100 per replica | 1-3 |

Both ScaledObjects use Azure Workload Identity for authentication against the Prometheus endpoint. The polling interval is 30 seconds for Bolt and 15 seconds for the API. Cooldown periods are 60 seconds and 30 seconds respectively.

### Container Apps configuration

Key Bicep parameters in `infra/main.bicep`:

| Parameter | Default | Description |
|-----------|---------|-------------|
| `boltMaxReplicas` | `1` | Maximum Bolt replicas. Must stay at 1 when `persistenceBackend` is `valkey-volume`. |
| `persistenceBackend` | `valkey-volume` | Backend mode: `valkey-volume`, `azure-cache`, or `azure-cache-cluster`. |

Stabilization: Container Apps uses a 60-second cooldown before scaling down, preventing flapping during load spikes. The KEDA configuration mirrors this with `cooldownPeriod: 60`.

### The valkey-volume guard

The Bicep template includes a compile-time assertion:

```
assert valkeyVolumeNoMultiReplica =
  !(persistenceBackend == 'valkey-volume' && boltMaxReplicas > 1)
```

**Why:** When using `valkey-volume` (Azure Files mount), the Valkey data container is a single-writer process. Azure Files does not support concurrent writes from multiple containers. Running multiple Bolt replicas is safe because they connect to Redis over the network, but the underlying Redis must be a shared service (Azure Cache) rather than a local file-backed container.

**To scale beyond one replica:** Switch `persistenceBackend` to `azure-cache` or `azure-cache-cluster`:

```bash
gh workflow run deploy.yml \
  -f environment=production \
  -f persistenceBackend=azure-cache \
  -f boltMaxReplicas=5
```

### Subnet sizing

The Container Apps subnet uses `/21` (2048 addresses) to provide headroom for KEDA scale-out. If upgrading from an older `/23` deployment, the VNet or subnet must be recreated or expanded before applying the new Bicep template.

## Write Safety Under Multi-Replica

When multiple Bolt replicas share the same Redis backend, concurrent writes to the same graph can conflict. SedanDelivery uses four mechanisms to prevent data corruption.

### 1. Per-graph distributed locks

Every write transaction acquires a distributed lock scoped to the target graph:

```
SET __sd2:lock:{graphname} <owner_id> NX PX 5000
```

- `NX` — only set if the key does not exist (mutex semantics).
- `PX 5000` — 5-second TTL auto-expires the lock if the holder crashes.
- **CAS release** — the lock is released with a Lua script that checks the owner value before deleting, preventing a replica from releasing another replica's lock.

```lua
if redis.call("GET", KEYS[1]) == ARGV[1] then
  return redis.call("DEL", KEYS[1])
else
  return 0
end
```

Write concurrency within a single replica is further bounded by `BOLT_WRITE_CONCURRENCY` (default 64), limiting the number of concurrent write transactions that can compete for locks.

### 2. Atomic delete_graph

Graph deletion uses a Lua script that atomically scans and deletes all keys matching the graph's key pattern. This prevents a window where a partially-deleted graph could be read by another replica:

```lua
-- Pseudocode: SCAN for G:{graph}:* keys and DEL them atomically
local cursor = "0"
repeat
  local result = redis.call("SCAN", cursor, "MATCH", KEYS[1], "COUNT", 1000)
  cursor = result[1]
  if #result[2] > 0 then redis.call("DEL", unpack(result[2])) end
until cursor == "0"
```

### 3. Redis-backed bookmark counter

Bolt bookmarks (transaction sequence numbers returned to clients for causal consistency) use a shared Redis counter:

```
INCR __sd2_bookmark
```

This ensures bookmark values are globally unique across replicas. When Redis is unavailable, the counter falls back to a per-process `AtomicU64` (which provides uniqueness within a single replica but not cross-replica ordering).

### 4. Read-after-write session pinning

When `GQL_REDIS_REPLICA_URL` is configured and reads are routed to replicas, a 5-second pinning window prevents stale reads after writes:

- Each Bolt session tracks `last_write_at` (wall-clock timestamp).
- For 5 seconds after any write (including DDL like CREATE INDEX), reads on that session are routed to the primary via `route_read_with_pinning(force_primary=true)`.
- After the window expires, reads resume normal replica round-robin routing.
- Pinning is per-session, not global. Other sessions without recent writes continue reading from replicas.

## Data Layer

### Per-graph changelog streams

When `GQL_FABRIC_CHANGELOG=1` (or `GQL_FABRIC_ENDPOINT` is set), the GQL module writes change events to per-graph Redis Streams:

```
Stream key:  __sd2:changelog:{graphname}
Example:     __sd2:changelog:{mydb}
```

The `{graphname}` hash tag ensures the stream lands in the same Redis Cluster slot as the graph's data keys. The legacy global stream `__sd2:changelog` is still consumed for backward compatibility during rolling upgrades but is no longer written to by new code.

The `sedandelivery-fabric-sync` sidecar discovers streams at startup via `SMEMBERS __sd2_registry:graphs` and issues a single multi-stream `XREADGROUP` command:

```
XREADGROUP GROUP sedandelivery-fabric-sync <consumer>
  COUNT 100 BLOCK 1000
  STREAMS __sd2:changelog __sd2:changelog:{graph_a} __sd2:changelog:{graph_b}
  > > >
```

### SMEMBERS to SSCAN for large sets

The `smembers_or_scan()` helper checks `SCARD` before reading a set. If the member count exceeds `GQL_REDIS_SMEMBERS_THRESHOLD` (default 10,000), it iterates with `SSCAN COUNT 1000` instead of a blocking `SMEMBERS`. This prevents a single large-set read from stalling the Redis event loop, which would block all other clients.

### Manual resharding only

Redis Cluster is supported for horizontal data sharding, but resharding is a manual operational procedure. There is no auto-scaling trigger for the data tier. This is documented in ADR-004. The reasons:

- All keys for a graph share a hash tag (`{graphname}`), so slot migration moves an entire graph at once.
- Slot migration under write load can cause transient `MOVED`/`ASK` redirections that the client handles transparently, but error rates spike during migration (validated by the resharding resilience chaos test).
- Automated resharding without operator oversight risks data availability issues.

To reshard manually:

```bash
redis-cli --cluster reshard <seed-node>:6379
```

## Observability

### Prometheus metrics

The Bolt server exposes metrics on the health port (default 7688) at `/metrics`:

| Metric | Type | Scaling relevance |
|--------|------|-------------------|
| `sedandelivery_bolt_connections_active` | Gauge | Primary KEDA trigger. High value means the replica is connection-saturated. |
| `sedandelivery_bolt_connections_total` | Counter | Trend indicator for capacity planning. |
| `sedandelivery_bolt_queries_total` | Counter | Labels: `type=read\|write`. Useful for read/write ratio analysis. |
| `sedandelivery_bolt_query_duration_seconds` | Histogram | p95 is a KEDA trigger. Sustained high latency signals CPU saturation. |
| `sedandelivery_bolt_errors_total` | Counter | Protocol errors by code. |
| `sedandelivery_bolt_backend_errors_total` | Gauge | `StorageError::Backend` count (Redis failures). Non-zero means the data tier is unhealthy. |
| `redis_pool_size` | Gauge | Total connections in the r2d2 pool. |
| `redis_pool_idle` | Gauge | Idle pool connections. Consistently zero means the pool is exhausted. |
| `redis_pool_active` | Gauge | Active (checked-out) pool connections. |
| `redis_pool_checkouts_total` | Counter | Successful pool checkouts. |
| `redis_pool_timeout_total` | Counter | Pool checkout timeouts. Non-zero signals Redis saturation or network issues. |
| `redis_used_memory_bytes` | Gauge | Redis `used_memory` from INFO. |
| `redis_maxmemory_bytes` | Gauge | Configured memory limit (0 = unlimited). |
| `redis_evicted_keys_total` | Counter | Cumulative evicted keys. Any increase is a warning. |

### Per-shard memory monitoring

For Redis Cluster deployments, each Bolt replica reports memory metrics from the shard it connects to. When using Azure Cache for Redis, the Azure Portal provides per-shard dashboards. For self-managed clusters, scrape each node's INFO output.

### Alert thresholds

Pre-built Prometheus alert rules are in `infra/prometheus-alert-rules.yaml`:

| Alert | Threshold | Severity | For |
|-------|-----------|----------|-----|
| `RedisEvictionOccurred` | Any key eviction in 5 minutes | Warning | 0m |
| `RedisMemoryPressureHigh` | `used_memory / maxmemory > 90%` | Warning | 5m |
| `RedisMemoryPressureCritical` | `used_memory / maxmemory > 95%` | Critical | 2m |

Deploy as a PrometheusRule CRD (Prometheus Operator) or load into Azure Monitor Managed Prometheus:

```bash
kubectl apply -f infra/prometheus-alert-rules.yaml
```

## Security Considerations

### Per-IP connection limits

Each Bolt replica enforces `BOLT_MAX_CONN_PER_IP` (default 50) independently. This is a per-replica counter, not a global one.

**Multi-replica implication:** With N replicas behind an L4 load balancer, a single IP can hold up to `N x BOLT_MAX_CONN_PER_IP` connections total. For typical deployments (1-3 replicas, default limit 50), this means 50-150 connections per IP, which is acceptable for most threat models.

### Auth rate limiting (per-replica)

The `BOLT_AUTH_MAX_ATTEMPTS` / `BOLT_AUTH_WINDOW_SECS` rate limiter is per-process. With N replicas, an attacker can make up to `N x max-attempts` total failed auth attempts before being throttled on every replica. For the default of 10 attempts with 3 replicas, worst-case is 30 attempts per window.

To fully centralize rate limiting, the counter would need to move to Redis (`INCR` with `EXPIRE` keyed by IP). This is deferred until a deployment requires tighter guarantees.

### Pre-handshake timeout

`BOLT_HANDSHAKE_TIMEOUT_MS` (default 5000ms) reclaims connection slots from clients that open a TCP connection but never complete the Bolt handshake. This prevents slowloris-style attacks from exhausting `BOLT_MAX_CONNECTIONS`.

### TCP keepalive vs Azure LB

TCP keepalives are sent every 120 seconds. Azure's L4 load balancer drops idle TCP connections after 240 seconds (4 minutes). The 120-second interval ensures the LB sees activity before its timeout, preventing silent connection resets.

### NSG CIDR configuration

The Bicep template provisions a Network Security Group on the Container Apps subnet. Inbound rules should use explicit CIDR ranges — avoid wildcard `0.0.0.0/0` rules for the Bolt port. Restrict access to known client IP ranges or use Azure Front Door for public endpoints.

## Testing

### Known Cypher limitations

`timestamp()` is not implemented in SedanDelivery. The load test uses plain integer IDs and batch counters instead of `timestamp()`. Do not use `timestamp()` in load tests or production queries — use application-generated timestamps or Redis-side counters.

### Running the load tests

```bash
# Build
cargo build --release -p loadtest

# Node creation (10K nodes, 50 concurrent)
./target/release/loadtest --bolt-uri bolt://localhost:7687 --scenario nodes --count 10000 --concurrency 50

# Connection ramp (to 1024)
./target/release/loadtest --bolt-uri bolt://localhost:7687 --scenario ramp --max-connections 1024 --duration 60

# Mixed read/write (80/20 ratio, 30s)
./target/release/loadtest --bolt-uri bolt://localhost:7687 --scenario mixed --concurrency 20 --duration 30 --ratio 80
```

### Running chaos tests

```bash
# Prerequisites
sudo apt-get install docker-compose-v2  # if not installed
export VALKEY_PASSWORD='your-password'

# Network partition test (toxiproxy)
bash scripts/test-network-partition.sh

# Scale-down drain test
bash scripts/test-scale-down-drain.sh

# Resharding resilience (requires Redis Cluster compose)
bash scripts/test-resharding-resilience.sh
```

### Running QA scenarios

```bash
# Start 2 Bolt instances + Redis, then:
python3 tests/qa/run_qa_scenarios.py --bolt1 bolt://localhost:7687 --bolt2 bolt://localhost:7689
```

### Baseline results

Results from live testing on the development environment:

| Test | Result |
|------|--------|
| Node creation (10K nodes, 50 concurrent) | 9,814 nodes/sec, p50 latency 512ms |
| Connection ramp to 1024 | Zero failures at all levels |
| Network partition (toxiproxy) | 5/5 phases pass |
| Scale-down drain | 6/6 checks pass |

### Multi-instance correctness test

Validates that two Bolt replicas sharing one Redis backend handle concurrent writes without data corruption:

```bash
python3 tests/test_multi_instance.py \
  --bolt1 bolt://localhost:7687 \
  --bolt2 bolt://localhost:7689 \
  --nodes-per-instance 200
```

Tests: concurrent node creation, concurrent edge creation, cross-instance read consistency, no orphaned edges or corrupted adjacency. Exit code 0 = all pass.

### QA scenario runner

Exercises write safety (B1), advanced scaling (B2), and observability (B4) scenarios against two live Bolt instances:

```bash
python3 tests/qa/run_qa_scenarios.py
```

Requires two Bolt instances on ports 7687 and 7689 sharing the same Redis (e.g., via `docker-compose.scale-test.yml`). Scenario files are in `tests/qa/`:

- `b1-write-safety.yaml` — concurrent property writes, no lost updates
- `b2-advanced-scaling.yaml` — replica scale-out behavior
- `b3-data-layer.yaml` — changelog streams, SSCAN thresholds
- `b4-observability.yaml` — metrics endpoints, alert rules

### Rust load test binary

A purpose-built Rust binary for validating performance under load:

```bash
cargo build -p loadtest --release
```

Three scenarios:

| Scenario | Command | What it tests |
|----------|---------|--------------|
| `nodes` | `--scenario nodes --count 100000 --concurrency 50` | Bulk node creation throughput and p50/p95/p99 latency |
| `ramp` | `--scenario ramp --max-connections 2000 --duration 60` | Connection ramp from 10 to max; asserts zero failures below `BOLT_MAX_CONNECTIONS` |
| `mixed` | `--scenario mixed --ratio 80 --concurrency 100 --duration 60` | Read/write mix (80/20 default) tracking errors and per-operation latency |

Output is JSON (default) or table format. See `tests/loadtest/README.md` for full usage.

### Chaos tests

Three scripts validate resilience under failure conditions. All require Docker Compose v2, Python 3 with the `neo4j` driver, and `VALKEY_PASSWORD` set.

```bash
export VALKEY_PASSWORD='test-password'
```

| Script | What it does |
|--------|-------------|
| `scripts/test-network-partition.sh` | Injects latency (500ms) and full disconnects between Bolt and Valkey via toxiproxy, then verifies recovery. Uses `docker-compose.chaos-test.yml`. |
| `scripts/test-resharding-resilience.sh` | Runs background write load while resharding Redis Cluster slots between masters. Checks `/metrics` for `StorageError::Backend` errors to detect data loss. Uses `docker-compose.cluster.yml`. |
| `scripts/test-scale-down-drain.sh` | Starts 3 Bolt instances, sends SIGTERM to one under load, verifies graceful shutdown with no connection resets and zero errors on survivors. |

## Rollback

To disable auto-scaling and return to a single replica:

```bash
az containerapp update \
  --name sd2-main-bolt \
  --resource-group rg-sd2-main \
  --min-replicas 1 \
  --max-replicas 1
```

Or set `boltMaxReplicas: 1` in the Bicep parameters and redeploy:

```bash
az deployment group create \
  --resource-group rg-sd2-main \
  --template-file infra/main.bicep \
  --parameters environmentName=main boltMaxReplicas=1
```

If KEDA ScaledObjects are applied, remove them first:

```bash
kubectl delete scaledobject sedandelivery-bolt-scaler -n sedandelivery
kubectl delete scaledobject gql-api-scaler -n sedandelivery
```

## Configuration Reference

All environment variables relevant to scaling behavior, in one table.

### Bolt Server

| Variable | Default | Description |
|----------|---------|-------------|
| `BOLT_MAX_CONNECTIONS` | `1024` | Max concurrent Bolt connections per replica |
| `BOLT_MAX_CONN_PER_IP` | `50` | Max connections from a single IP per replica |
| `BOLT_HANDSHAKE_TIMEOUT_MS` | `5000` | Max time to complete handshake before reclaiming slot |
| `BOLT_IDLE_TIMEOUT_SECS` | `300` | Close idle connections after N seconds |
| `BOLT_MAX_CONNECTION_LIFETIME_SECS` | `1800` | Force-close connections after N seconds (0 = disabled) |
| `BOLT_WRITE_CONCURRENCY` | `64` | Max concurrent write transactions per replica |
| `BOLT_AUTH_MAX_ATTEMPTS` | `10` | Max failed auth attempts per IP per window |
| `BOLT_AUTH_WINDOW_SECS` | `60` | Auth rate limit sliding window (seconds) |

### Redis Connection Pool

| Variable | Default | Description |
|----------|---------|-------------|
| `GQL_REDIS_URL` | (none) | Redis/Valkey connection URL. Required for multi-replica. |
| `GQL_REDIS_CLUSTER_NODES` | (none) | Cluster seed URLs (overrides `GQL_REDIS_URL`) |
| `GQL_REDIS_REPLICA_URL` | (none) | Comma-separated replica URLs for read routing |
| `GQL_REDIS_POOL_SIZE` | `64` | r2d2 connection pool max size |
| `GQL_REDIS_POOL_TIMEOUT_SECS` | `5` | Pool checkout timeout (seconds) |
| `GQL_REDIS_IDLE_TIMEOUT_SECS` | `300` | Idle connection reap timeout (seconds) |
| `GQL_REDIS_MIN_IDLE` | `1` | Minimum warm connections in pool |
| `GQL_REDIS_KEY_PREFIX` | (none) | Key prefix for multi-tenant isolation |
| `GQL_REDIS_SMEMBERS_THRESHOLD` | `10000` | Member count above which SSCAN replaces SMEMBERS |

### REST API

| Variable | Default | Description |
|----------|---------|-------------|
| `GQL_API_MAX_CONCURRENCY` | `100` | Max concurrent HTTP requests per replica (excess gets 503) |
| `GQL_API_RATE_LIMIT_RPS` | `100` | Max requests per second per client |

### Fabric Sync

| Variable | Default | Description |
|----------|---------|-------------|
| `GQL_FABRIC_CHANGELOG` | `0` | Set to `1` to enable per-graph changelog streams |
| `SD2_SYNC_BATCH` | `100` | Entries per XREADGROUP batch |
| `SD2_SYNC_BLOCK_MS` | `1000` | XREADGROUP block timeout (ms) |
| `SD2_SYNC_MAX_RETRIES` | `3` | Max retries per event before DLQ |
| `SD2_SYNC_CONSUMER` | hostname | Consumer name within the group |

### Infrastructure (Bicep)

| Parameter | Default | Description |
|-----------|---------|-------------|
| `boltMaxReplicas` | `1` | Max Bolt replicas. Must be 1 for `valkey-volume` backend. |
| `persistenceBackend` | `valkey-volume` | `valkey-volume`, `azure-cache`, or `azure-cache-cluster` |
| `clusterNodeCount` | `3` | Cluster shards (only for `azure-cache-cluster`) |
| `replicaCount` | `0` | Read replicas per shard |
