# Performance Benchmarks

SedanDelivery ships three Criterion benchmark suites in `gql-core/benches/`.
They establish reproducible performance baselines that CI tracks for regressions.

## Running benchmarks

```bash
# Run all three suites (takes ~2–5 min):
cargo bench -p gql-core

# Run a single suite:
cargo bench -p gql-core --bench parser
cargo bench -p gql-core --bench executor
cargo bench -p gql-core --bench scale

# Run a single named benchmark (substring match):
cargo bench -p gql-core --bench executor -- node_scan
cargo bench -p gql-core --bench executor -- e2e_friends_of_friends

# Generate an HTML report (opens in browser):
cargo bench -p gql-core -- --output-format html
# Report is written to target/criterion/report/index.html
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

## Saving and comparing baselines

```bash
# Save the current results as the baseline named "main":
cargo bench -p gql-core -- --save-baseline main

# Later, compare against the saved baseline:
cargo bench -p gql-core -- --baseline main

# Criterion will flag regressions with "Performance has regressed":
# node_scan/1000   change: [+12.34% +13.01% +13.70%] (p = 0.00)
#                  Performance has regressed.
```

Baselines are stored in `target/criterion/`.  They are excluded from git.
To track baselines across machines or CI runs, archive the
`target/criterion/` directory as a build artifact.

## CI integration

The `Performance Benchmarks` job runs on every push to `main` and uploads
results as a GitHub Actions artifact named `benchmark-results`.

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
cargo bench -p gql-core -- --output-format bencher 2>&1 | tee current.txt

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
