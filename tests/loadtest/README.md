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
  --count 100000 \
  --concurrency 50
```

| Flag | Default | Description |
|------|---------|-------------|
| `--count` | `100000` | Total nodes to create |
| `--concurrency` | `50` | Parallel connections |

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

Runs a configurable read/write mix for a fixed duration, tracking errors (including backend/storage errors) and per-operation latency.

```bash
cargo run -p loadtest -- \
  --bolt-uri bolt://localhost:7687 \
  --scenario mixed \
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
| `--format` | `json` | Output format: `json` or `table` |

## Output

Results are printed to stdout as JSON (default) or a human-readable table. Example JSON output for the nodes scenario:

```json
{
  "scenario": "nodes",
  "total_nodes": 100000,
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

## Known Limitations

- **`timestamp()` is not implemented in SedanDelivery.** The load test uses plain integer IDs and batch counters instead. Do not use `timestamp()` in load test queries or production Cypher queries.
- **Auth defaults to neo4j/neo4j** when `--user` and `--password` are omitted. If the Bolt server requires authentication, pass credentials explicitly.

## Logging

Set the `RUST_LOG` environment variable to control log verbosity:

```bash
RUST_LOG=debug cargo run -p loadtest -- --scenario nodes
```
