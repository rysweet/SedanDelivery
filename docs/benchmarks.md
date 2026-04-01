# Performance Benchmarks

SedanDelivery now ships four benchmark surfaces:

- `cypher-core/benches/` for parser/planner/executor baselines
- `gql-redis-store/benches/` for Redis safety-tax baselines
- `sedandelivery-bolt/benches/` for Bolt wire and routing microbenchmarks
- `tests/loadtest/` for outside-in topology comparisons over the public Bolt path

## Running benchmarks

### `cypher-core` engine suites

```bash
# Run all three suites (takes ~2–5 min):
cargo bench -p cypher-core

# Run a single suite:
cargo bench -p cypher-core --bench parser
cargo bench -p cypher-core --bench executor
cargo bench -p cypher-core --bench scale

# Run a single named benchmark (substring match):
cargo bench -p cypher-core --bench executor -- node_scan
cargo bench -p cypher-core --bench executor -- e2e_friends_of_friends

# Generate an HTML report (opens in browser):
cargo bench -p cypher-core -- --output-format html
# Report is written to target/criterion/report/index.html
```

### `gql-redis-store` safety-tax suite

```bash
# Standalone Redis/Valkey:
export VALKEY_PASSWORD='change-me-for-local-dev'
docker compose up -d --build
GQL_REDIS_URL="redis://:${VALKEY_PASSWORD}@127.0.0.1:6379" \
  cargo bench -p gql-redis-store --bench store_safety

# Cluster mode: keep the cluster inside Docker, then run the bench from a
# disposable Rust container attached to the compose network.
export COMPOSE_PROJECT_NAME=benchcluster
export VALKEY_PASSWORD='change-me-for-local-dev'
docker compose -f docker-compose.cluster.yml up -d --build
docker run --rm \
  --network "${COMPOSE_PROJECT_NAME}_default" \
  -v "$PWD":/workspace \
  -w /workspace \
  -e STORE_SAFETY_ALLOW_REMOTE=1 \
  -e GQL_REDIS_CLUSTER_NODES="redis://:${VALKEY_PASSWORD}@redis-node-1:6379,redis://:${VALKEY_PASSWORD}@redis-node-2:6379,redis://:${VALKEY_PASSWORD}@redis-node-3:6379,redis://:${VALKEY_PASSWORD}@redis-node-4:6379,redis://:${VALKEY_PASSWORD}@redis-node-5:6379,redis://:${VALKEY_PASSWORD}@redis-node-6:6379" \
  rust:bookworm bash -lc 'cargo bench -p gql-redis-store --bench store_safety'
```

`store_safety` refuses non-local targets by default because it repeatedly
creates and deletes graphs. The cluster command above opts in with
`STORE_SAFETY_ALLOW_REMOTE=1` because the targets are isolated inside the
compose network, not on the public host.

### `sedandelivery-bolt` routing microbenchmarks

```bash
cargo bench -p sedandelivery-bolt --bench bolt_benchmarks -- read_consistency
```

### Outside-in topology baselines with `loadtest`

```bash
# Single-graph write throughput (same command shape works against standalone or cluster)
./target/release/loadtest \
  --bolt-uri bolt://127.0.0.1:7687 \
  --user neo4j \
  --password "${VALKEY_PASSWORD}" \
  --scenario nodes \
  --graph benchmark_nodes \
  --count 20000 \
  --concurrency 1 \
  --format json

# Stable mixed read/write comparison
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

Criterion produces output like:

```
node_scan/1000       time:   [123.45 µs  124.12 µs  124.87 µs]
                     change: [-1.23% -0.85% -0.41%] (p = 0.00 < 0.05)
                     Performance has improved.
```

The three numbers are the lower bound, median, and upper bound of the
confidence interval.

## What each suite covers

### `benches/parser.rs` — tokeniser and parser throughput

| Benchmark | Measures |
|-----------|---------|
| `tokenize_simple_query` | Lexer throughput on a simple MATCH/WHERE/RETURN |
| `parse_simple_match_return` | Parser throughput on a short query |
| `parse_complex_multi_clause` | MATCH + WHERE + WITH + ORDER BY + LIMIT |
| `parse_create_statement` | CREATE with two nodes and one relationship |
| `parse_deeply_nested_50` | Parser on 50 levels of nested arithmetic |
| `tokenize_throughput` | Raw tokeniser bytes/sec over a long query |
| `parse_merge_with_on_clauses` | MERGE + ON CREATE SET + ON MATCH SET |

### `benches/executor.rs` — operator execution throughput

| Benchmark | Measures |
|-----------|---------|
| `node_scan/{100,1000,10000}` | Label scan at three graph sizes |
| `filter_equality_1k` | `WHERE n.age = 42` on 1 000 nodes |
| `filter_range_1k` | `WHERE n.age > 50` on 1 000 nodes |
| `filter_starts_with_1k` | `WHERE n.name STARTS WITH 'x'` on 1 000 nodes |
| `sort/{100,1000}` | `ORDER BY name` at two sizes |
| `aggregate_count_group_by_1k` | `RETURN age, count(n)` on 1 000 nodes |
| `expand_star_1000` | 1 hub node → 1 000 leaf expansions |
| `expand_chain_500` | 500-node chain, single-hop traverse |
| `e2e_friends_of_friends` | 2-hop social query on 100-node ring graph |
| `e2e_full_text_like` | `STARTS WITH` query on 1 000 nodes |
| `e2e_recommendation_200` | Aggregation + sort + LIMIT on 200 nodes |
| `create_100_nodes` | `UNWIND range(1,100) CREATE` throughput |

### `benches/scale.rs` — large-graph baselines

| Benchmark | Measures |
|-----------|---------|
| `load_nodes/{1000,10000}` | Storage write throughput |
| `label_scan/{1000,10000}` | `MATCH (n:Node) RETURN count(n)` at scale |
| `all_node_ids/{1000,10000}` | `all_node_ids()` storage method |
| `e2e_path_1hop_200` | 1-hop traversal on 200-node chain |
| `e2e_filter_5k` | Selective filter (5% hit rate) on 5 000 nodes |
| `e2e_aggregation_3k` | Group-by on 3 000 nodes, 10 buckets |

### `gql-redis-store/benches/store_safety.rs` — post-#745 safety-tax paths

| Benchmark | Measures |
|-----------|---------|
| `store_safety_lock/create_graph/{standalone,cluster}` | Graph-scoped lock acquisition/release for empty graph creation |
| `store_safety_lock/create_node_single_label/{standalone,cluster}` | One lock-protected node write with the lightest registry update path |
| `store_safety_lock/create_node_multi_label/{standalone,cluster}` | Node write with multi-label registry churn |
| `store_safety_index/{standalone,cluster}/1000` | Index build below the default `SMEMBERS`/`SSCAN` pivot |
| `store_safety_index/{standalone,cluster}/12000` | Index build above the default scan threshold |
| `store_safety_delete/delete_graph/{standalone,cluster}` | Delete over a graph with nodes, an index manifest, and edge teardown |

### `sedandelivery-bolt/benches/bolt_benchmarks.rs` — routing decision cost

| Benchmark | Measures |
|-----------|---------|
| `read_consistency_no_recent_write` | Session routing when no write-backed freshness window exists |
| `read_consistency_recent_write` | Fast path that pins reads to the primary |
| `read_consistency_window_expired` | Expired freshness window returning to topology-default routing |

### `tests/loadtest` — public-path topology evidence

| Scenario | Measures |
|----------|----------|
| `nodes --count 20000 --concurrency 1` | Same-graph write throughput and end-to-end write latency |
| `mixed --ratio 80 --concurrency 8 --duration 20` | Stable read/write throughput split plus read/write p99 latency |

## Local baseline snapshot

These baselines came from local Docker runs against the benchmark commands
above. Treat them as comparison anchors, not hard gates.

### Store safety medians

| Benchmark | Standalone median | Cluster median |
|-----------|-------------------|----------------|
| `create_graph` | `566.91 µs` | `670.58 µs` |
| `create_node_single_label` | `1.1301 ms` | `1.0585 ms` |
| `create_node_multi_label` | `1.0969 ms` | `1.0686 ms` |
| `create_index/1000` | `9.0293 ms` | `11.571 ms` |
| `create_index/12000` | `112.63 ms` | `129.58 ms` |
| `delete_graph` | `32.670 ms` | `30.829 ms` |

The issue `#758` validator and Gadugi scenario intentionally spot-check the
representative `create_graph` and `create_index/1000` IDs in each topology to
keep the outside-in run bounded. The full six-path `store_safety` matrix remains
available as a manual/advisory Criterion surface for deeper baseline collection.

### Bolt routing microbench medians

| Benchmark | Median |
|-----------|--------|
| `read_consistency_no_recent_write` | `10.613 ns` |
| `read_consistency_recent_write` | `2.7804 ns` |
| `read_consistency_window_expired` | `2.7863 ns` |

The routing decision itself is effectively free compared with network or Redis
latency; the meaningful topology cost shows up in the outside-in load tests.

### Outside-in topology baselines

| Scenario | Standalone | Cluster |
|----------|------------|---------|
| `nodes` (`count=20000`, `concurrency=1`) | `1544.8 nodes/sec`, `p99=83.12 ms`, `errors=0` | `1102.7 nodes/sec`, `p99=103.22 ms`, `errors=0` |
| `mixed` (`ratio=80`, `concurrency=8`, `duration=20`) | `21.02 ops/sec` (`17.05` reads/sec, `3.97` writes/sec), `read p99=566.41 ms`, `write p99=55.30 ms`, `errors=0` | `13.00 ops/sec` (`11.03` reads/sec, `1.97` writes/sec), `read p99=752.24 ms`, `write p99=105.78 ms`, `errors=0` |

## Saving and comparing baselines

```bash
# Save the current results as the baseline named "main":
cargo bench -p cypher-core -- --save-baseline main

# Later, compare against the saved baseline:
cargo bench -p cypher-core -- --baseline main

# Criterion will flag regressions with "Performance has regressed":
# node_scan/1000   change: [+12.34% +13.01% +13.70%] (p = 0.00)
#                  Performance has regressed.
```

Baselines are stored in `target/criterion/`.  They are excluded from git.
To track baselines across machines or CI runs, archive the
`target/criterion/` directory as a build artifact.

## Outside-in safety validator (`test-safety-benchmarking.sh`)

`scripts/test-safety-benchmarking.sh` is the outside-in validator for Issue
#758. It spins up isolated Docker environments and verifies all four benchmark
surfaces in sequence.

### Running the validator

```bash
# All defaults — spins up standalone + cluster Docker environments
./scripts/test-safety-benchmarking.sh

# Override concurrency and duration for the mixed scenario
MIXED_CONCURRENCY=16 MIXED_DURATION=20 ./scripts/test-safety-benchmarking.sh

# Override the node count for the nodes loadtest scenario
NODES_COUNT=5000 ./scripts/test-safety-benchmarking.sh
```

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `VALKEY_PASSWORD` | `issue-758-local-pass` | Password for Valkey and Bolt auth in the test environments |
| `BOLT_PASSWORD` | `$VALKEY_PASSWORD` | Overrides the Bolt password independently of Valkey |
| `NODES_COUNT` | `2000` | Node count passed to the `nodes` loadtest scenario |
| `MIXED_DURATION` | `8` | Duration (seconds) for the `mixed` loadtest scenario |
| `MIXED_CONCURRENCY` | `8` | Worker concurrency for the `mixed` loadtest scenario |

### Timeout contract

All `loadtest` invocations are wrapped by `run_capture`, which enforces
a hard ceiling on how long each invocation may run:

| Scenario | Timeout budget | Rationale |
|----------|---------------|-----------|
| `nodes` (standalone and cluster) | 180 s | Fixed; dataset is bounded by `NODES_COUNT`, but still gets generous startup headroom |
| `mixed` (standalone and cluster) | `MIXED_DURATION + 30` s | 30 s headroom on top of the configured duration for connection/setup variance |

If any invocation exceeds its budget, `timeout(1)` sends `SIGTERM` and returns
exit code **124**. The script treats exit code 124 as a **hard failure**:

```
[FAIL] Command timed out after <N>s: ...
```

The captured output file is dumped to stderr and the script exits non-zero. This
is intentional — a timeout in CI means either the server stalled or the test
environment was overloaded. Both conditions require investigation, not a silent
pass.

Exit codes other than 0 and 124 are also hard failures. Only a clean exit (0)
means the loadtest ran to completion with its assertions satisfied.

### `GQL_REDIS_CLUSTER_NODES` validation

The `store_safety` benchmark suite (`gql-redis-store/benches/store_safety.rs`)
reads `GQL_REDIS_CLUSTER_NODES` to choose between standalone and cluster mode.
Before attempting to connect, it validates the variable:

1. **Locality check** — each node URL must resolve to `127.0.0.1`, `localhost`,
   or `::1` unless `STORE_SAFETY_ALLOW_REMOTE=1` is set. This prevents the
   destructive benchmark (which creates and deletes graphs repeatedly) from
   accidentally targeting a non-local environment.

2. **Empty-node guard** — if `GQL_REDIS_CLUSTER_NODES` is set but contains no
   valid node URLs after splitting and trimming (e.g. the variable is set to
   whitespace or a string of bare commas), the benchmark fails immediately with:

   ```
   Skipping store_safety benchmarks: GQL_REDIS_CLUSTER_NODES is set but
   contains no valid node URLs after parsing
   ```

   An empty list passed to `ensure_local_urls` would silently skip locality
   validation. Failing early prevents a misconfigured `GQL_REDIS_CLUSTER_NODES`
   from surfacing later as a misleading Redis connection/configuration error.

   Set `GQL_REDIS_CLUSTER_NODES` to a proper comma-separated list of
   `redis://[user:pass@]host:port` URLs, or leave it unset to use standalone
   mode via `GQL_REDIS_URL`.

### What the validator checks

```
[PASS] Bolt routing benchmark reported read_consistency_recent_write
[PASS] Standalone store benchmark emitted create_graph/standalone (median …)
[PASS] Standalone store benchmark emitted index/standalone/1000 (median …)
[PASS] Standalone nodes loadtest reported count integrity and positive throughput
[PASS] Standalone mixed loadtest reported throughput split with zero errors
[PASS] Cluster store benchmark emitted create_graph/cluster (median …)
[PASS] Cluster store benchmark emitted index/cluster/1000 (median …)
[PASS] Cluster nodes loadtest reported count integrity and positive throughput
[PASS] Cluster mixed loadtest reported throughput split with zero errors
[PASS] Safety benchmarking validator passed
```

The script exits non-zero on the first failure. Partial output indicates which
surface failed and which had not yet run.

## CI integration

The `Performance Benchmarks` job runs on every push to `main` and uploads
results as a GitHub Actions artifact named `benchmark-results`.

That job currently covers the stable `cypher-core` suites only. The Redis
`store_safety` and outside-in load tests remain manual/advisory for now; they
should inform reviews, but they are not required merge gates until their
baseline variance is better understood.

To download the latest results:

```bash
gh run list --workflow ci.yml --branch main --limit 1
gh run download <run-id> --name benchmark-results
```

The downloaded files (`bench-parser.txt`, `bench-executor.txt`,
`bench-scale.txt`) are in `--output-format bencher` (one line per benchmark,
compatible with tools like [Bencher](https://bencher.dev)).

## Regression detection

Criterion's built-in comparison (`--baseline`) detects regressions per run.
For automated CI alerting, compare the current run's bencher output against
the last `main` artifact:

```bash
# Download the baseline (last successful main run)
gh run download --name benchmark-results --dir baseline/

# Run benchmarks and save output
cargo bench -p cypher-core -- --output-format bencher 2>&1 | tee current.txt

# Compare: lines where % change > 10%
diff baseline/bench-executor.txt current.txt   # rough comparison
```

A structured CI step using the [Bencher GitHub Action](https://bencher.dev/docs/how-to/github-actions/)
can automate this and post PR comments when regressions exceed 10%.

## Interpreting results

- **p-value < 0.05**: the change is statistically significant.
- **Change within ±5%**: noise — ignore.
- **Change > +10%**: investigate before merging.  Run the specific bench with
  `-- --sample-size 200` for higher confidence.
- **Change < -5%**: improvement — note in PR description.
- For `loadtest mixed`, compare `total_ops_per_sec`, `reads_per_sec`, and
  `writes_per_sec` together. A flat total with rising write latency usually
  means the graph write lock, not the routing policy, is the limiter.
- Use `--concurrency 8` for stable topology comparisons. Higher mixed-workload
  concurrency mostly measures lock contention and can produce standalone write
  conflicts even when the system is behaving correctly.

## Adding new benchmarks

Add functions to the appropriate file, register them in `criterion_group!`,
and follow the existing naming convention: `bench_<what>_<size_or_detail>`.

```rust
fn bench_my_operator(c: &mut Criterion) {
    let store = setup_store_with_nodes(1_000);
    c.bench_function("my_operator_1k", |b| {
        b.iter(|| run_query(&store, "MATCH (n:Node) WHERE n.x > 5 RETURN count(n)"))
    });
}

criterion_group!(benches, ..., bench_my_operator);
```
