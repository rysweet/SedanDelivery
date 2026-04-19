# scalability-tests

Large-data scalability tests for the SedanDelivery embedded engines:

- `sedan-cypher-embedded` (openCypher)
- `sedan-gql-embedded` (GQL/Cypher)
- `sedan-gremlin-embedded` (Apache TinkerPop Gremlin)

Each engine is exercised against `InMemoryGraphStore` with three scenarios:

| Scenario | Dataset | Query | Expected count |
|---|---|---|---|
| Linear chain | 100 000 nodes, 99 999 `:NEXT` edges | 5-hop path count | 99 995 |
| Dense graph | 10 000 nodes, 500 000 random edges | distinct 2-hop reachable from `:Seed` | computed from RNG |
| Property scan | 100 000 nodes with mixed properties | `weight > 50 000 AND parity = true` | 24 999 |

Every test asserts:

1. **Correctness** — exact result count.
2. **Soft wall-clock budget** — the run must complete inside
   `BUDGET` (60 s by default; see `src/lib.rs`). Crossing the budget
   fails the test with a clear message so a regression cannot hide as a
   silent slowdown.

## Running

The scenarios are marked `#[ignore]` so the default `cargo test` stays
fast. Run the full suite with:

```bash
cargo test -p scalability-tests --release -- --ignored --nocapture
```

`--release` is strongly recommended; debug builds may exceed the soft
budget without indicating any real regression.

To run a single engine:

```bash
cargo test -p scalability-tests --release --test cypher_scale  -- --ignored --nocapture
cargo test -p scalability-tests --release --test gql_scale     -- --ignored --nocapture
cargo test -p scalability-tests --release --test gremlin_scale -- --ignored --nocapture
```

## Determinism

The dense graph generator uses an inline xorshift64\* PRNG seeded with a
fixed constant (`DENSE_RNG_SEED` in `src/lib.rs`). The expected
two-hop count is recomputed from the same RNG stream during seeding —
if the generator changes, the expected value updates with it, so the
test never silently lies about correctness.

## Adding scenarios

1. Add a generator (and, if needed, an expected-count helper) to
   `src/lib.rs`. Keep the helper deterministic.
2. Add a `#[test] #[ignore]` function to each of `tests/cypher_scale.rs`,
   `tests/gql_scale.rs`, and `tests/gremlin_scale.rs` that calls the
   generator, runs the query, asserts the count, and wraps the work in
   `timed(...)` for budget enforcement.
