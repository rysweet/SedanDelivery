# Load Test Tool

Rust binary for validating SedanDelivery under realistic scale. Connects directly to a Bolt endpoint using the `neo4rs` crate.

## Prerequisites

- Rust 1.70+ (install via [rustup](https://rustup.rs/))
- A running SedanDelivery Bolt endpoint (see [DEPLOY.md](../../docs/DEPLOY.md) or `docker compose up -d` from the repo root)

## Building

```bash
cargo build -p loadtest --release
```

The binary is placed at `target/release/loadtest`.

## Scenarios

### 1. Node Creation Scale (`--scenario nodes`)

Creates a configurable number of nodes across concurrent connections, verifies the final count, and reports throughput (nodes/sec) with p50/p95/p99 latency.

```bash
cargo run -p loadtest -- \
  --bolt-uri bolt://localhost:7687 \
  --scenario nodes \
  --graph loadtest_nodes \
  --count 100000 \
  --concurrency 1 \
  --min-throughput 100 \
  --max-p99-ms 30000 \
  --max-error-rate 0.01
```

| Flag | Default | Description |
|------|---------|-------------|
| `--count` | `100000` | Total nodes to create |
| `--concurrency` | `50` | Parallel connections (use `1` for same-graph bulk-write throughput evidence) |

### 2. Connection Ramp (`--scenario ramp`)

Ramps connections from 10 up to `--max-connections` over `--duration` seconds. Asserts zero failures at or below `BOLT_MAX_CONNECTIONS` (1024).

```bash
cargo run -p loadtest -- \
  --bolt-uri bolt://localhost:7687 \
  --scenario ramp \
  --max-connections 2000 \
  --duration 60
```

| Flag | Default | Description |
|------|---------|-------------|
| `--max-connections` | `2000` | Upper bound of the ramp |
| `--duration` | `60` | Ramp duration in seconds |

### 3. Mixed Read/Write (`--scenario mixed`)

Runs a configurable read/write mix for a fixed duration, tracking errors
(including backend/storage errors), per-operation latency, and aggregate
throughput split into `total_ops_per_sec`, `reads_per_sec`, and
`writes_per_sec`.

```bash
cargo run -p loadtest -- \
  --bolt-uri bolt://localhost:7687 \
  --scenario mixed \
  --graph loadtest_mixed \
  --ratio 80 \
  --concurrency 100 \
  --duration 60
```

| Flag | Default | Description |
|------|---------|-------------|
| `--ratio` | `80` | Read percentage (0-100); remainder is writes |
| `--concurrency` | `100` | Parallel workers |
| `--duration` | `60` | Test duration in seconds |

## Common Options

| Flag | Default | Description |
|------|---------|-------------|
| `--bolt-uri` | `bolt://localhost:7687` | Bolt endpoint URL |
| `--user` | (empty) | Bolt auth username |
| `--password` | (empty) | Bolt auth password |
| `--graph` | `loadtest` | Named graph created for the scenario and dropped on cleanup |
| `--format` | `json` | Output format: `json` or `table` |
| `--skip-cleanup` | `false` | Leave the graph in place after printing results; useful when the surrounding environment will be torn down immediately afterward |
| `--min-throughput` | unset | Fail with exit code `2` if the `nodes` scenario throughput (nodes/sec) is below this value |
| `--max-p99-ms` | unset | Fail with exit code `2` if p99 latency exceeds this value |
| `--max-error-rate` | unset | Fail with exit code `2` if the scenario error rate exceeds this value |

## Output

Results are printed to stdout as JSON (default) or a human-readable table. Example JSON output for the nodes scenario:

```json
{
  "scenario": "nodes",
  "total_nodes": 100000,
  "created_nodes": 100000,
  "verified_count": 100000,
  "count_matches": true,
  "elapsed_secs": 12.34,
  "throughput_nodes_per_sec": 8103.7,
  "concurrency": 50,
  "latency": {
    "p50_ms": 4.2,
    "p95_ms": 12.1,
    "p99_ms": 18.7,
    "min_ms": 1.1,
    "max_ms": 45.3,
    "mean_ms": 5.8
  },
  "errors": 0
}
```

Example JSON output for the mixed scenario:

```json
{
  "scenario": "mixed",
  "duration_secs": 20.17,
  "concurrency": 8,
  "read_ratio": 80,
  "total_reads": 344,
  "total_writes": 80,
  "total_ops": 424,
  "total_ops_per_sec": 21.02,
  "reads_per_sec": 17.05,
  "writes_per_sec": 3.97,
  "read_errors": 0,
  "write_errors": 0,
  "backend_errors": 0,
  "read_latency": {
    "p50_ms": 454.11,
    "p95_ms": 555.93,
    "p99_ms": 566.41,
    "min_ms": 393.71,
    "max_ms": 605.89,
    "mean_ms": 466.12
  },
  "write_latency": {
    "p50_ms": 4.29,
    "p95_ms": 54.86,
    "p99_ms": 55.30,
    "min_ms": 2.26,
    "max_ms": 55.30,
    "mean_ms": 10.43
  }
}
```

When threshold flags are provided, the binary still prints the JSON/table
result first and then exits with status `2` if any assertion fails. That makes
it usable both for ad hoc evidence collection and for local or custom scripted
regression gates.

The `nodes` and `mixed` scenarios normally drop the named graph after printing
results. Pass `--skip-cleanup` when the graph lives inside an ephemeral test
environment that will be destroyed immediately after the run, such as the
issue `#758` validator's temporary Docker Compose stacks.

For the `nodes` scenario, all writes target one graph and therefore serialize
on the graph write lock. Use `--concurrency 1` for clean 100K evidence runs;
higher values are still allowed, but they measure lock contention more than
steady-state bulk-write throughput.

For the `mixed` scenario, use `--concurrency 8` for stable standalone-vs-cluster
comparisons. Higher worker counts are useful when you want to study contention,
but they can start surfacing write conflicts on standalone because every worker
is still targeting one graph-level write lock.

The mixed workload now uses a shared global operation counter when deciding
read versus write operations. That keeps the aggregate `--ratio` honest even
when each worker only completes a small number of operations during the test.

## Topology comparison recipe

These are the command shapes used for the current local baselines:

```bash
# Standalone same-graph write throughput
./target/release/loadtest \
  --bolt-uri bolt://127.0.0.1:7687 \
  --user neo4j \
  --password "${VALKEY_PASSWORD}" \
  --scenario nodes \
  --graph benchmark_nodes \
  --count 20000 \
  --concurrency 1 \
  --format json

# Stable mixed topology comparison
./target/release/loadtest \
  --bolt-uri bolt://127.0.0.1:7687 \
  --user neo4j \
  --password "${VALKEY_PASSWORD}" \
  --scenario mixed \
  --graph benchmark_mixed \
  --ratio 80 \
  --concurrency 8 \
  --duration 20 \
  --format json
```

Current local Docker baselines:

| Scenario | Standalone | Cluster |
|----------|------------|---------|
| `nodes` (`count=20000`, `concurrency=1`) | `1544.8 nodes/sec`, `p99=83.12 ms`, `errors=0` | `1102.7 nodes/sec`, `p99=103.22 ms`, `errors=0` |
| `mixed` (`ratio=80`, `concurrency=8`, `duration=20`) | `21.02 ops/sec` (`17.05` reads/sec, `3.97` writes/sec), `read p99=566.41 ms`, `write p99=55.30 ms`, `errors=0` | `13.00 ops/sec` (`11.03` reads/sec, `1.97` writes/sec), `read p99=752.24 ms`, `write p99=105.78 ms`, `errors=0` |

## Known Limitations

- **`timestamp()` is not implemented in SedanDelivery.** The load test uses plain integer IDs and batch counters instead. Do not use `timestamp()` in load test queries or production Cypher queries.
- **Auth defaults to neo4j/neo4j** when `--user` and `--password` are omitted. If the Bolt server requires authentication, pass credentials explicitly.

## Logging

Set the `RUST_LOG` environment variable to control log verbosity:

```bash
RUST_LOG=debug cargo run -p loadtest -- --scenario nodes
```
