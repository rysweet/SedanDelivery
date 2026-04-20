# concurrent-stress

Concurrent stress harness for the SedanDelivery embedded engines
(`sedan-cypher-embedded`, `sedan-gql-embedded`, `sedan-gremlin-embedded`).

Each engine is hammered by configurable reader/writer thread counts against
a single `Arc<InMemoryGraphStore>`, asserting:

* **No panics** — worker `JoinHandle::join()` propagates panics into the test.
* **No deadlocks** — every scenario terminates within its configured budget.
* **No data corruption** — write-heavy scenarios verify that the post-run
  `:Person` node count equals `seed_nodes + successful_writes`.

## Scenarios (per engine)

| Scenario      | Readers | Writers | Duration | Asserts                                       |
| ------------- | ------: | ------: | -------- | --------------------------------------------- |
| `read-heavy`  |       8 |       1 | 5 s      | every read returns valid (possibly stale) data |
| `write-heavy` |       1 |       8 | 5 s      | final node count == expected                  |
| `mixed`       |       4 |       4 | 5 s      | no panics, no deadlocks                       |
| `burst`       |    32 spawn-and-join | — | one query each, no panics |

Default config: 4 scenarios × 5 s × 3 engines ≈ 60 s wall-clock.

All write payloads use a deterministic per-thread seed
(`tid * 0xBF58_476D_1CE4_E5B9 + counter`) so failures reproduce.

## Running

```bash
# Default suite — runs in ~60 s
cargo test -p concurrent-stress

# Single engine
cargo test -p concurrent-stress --test cypher
cargo test -p concurrent-stress --test gql
cargo test -p concurrent-stress --test gremlin

# Soak (60 s per scenario, ~12 minutes total) — for nightly runs
cargo test -p concurrent-stress -- --ignored --nocapture

# See the per-scenario ops/sec line
cargo test -p concurrent-stress -- --nocapture
```

## Reporting bugs

If a scenario panics, deadlocks, or fails the consistency assertion, file
a separate issue with:

1. The engine and scenario name (printed on the `[stress]` line).
2. The thread count and seed values (deterministic — included in the panic
   message via `thread::Builder::name`).
3. Output of `cargo test -p concurrent-stress -- --nocapture`.
