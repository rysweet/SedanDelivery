# Validation Guide

This document explains every layer of automated validation for SedanDelivery,
what each layer proves, and how to run it — both locally and in CI.

In addition to the Rust and module suites, CI and `scripts/validate.sh` also
run validation for the React/Vite web app in
`sedan-graph-ui/src/sedan-web`.

The work described here was established in
[issue #189](https://github.com/cloud-ecosystem-security/SedanDelivery/issues/189).

---

## Quick reference

```bash
# Full local validation (takes ~5 min — runs everything except benchmarks)
bash scripts/validate.sh

# Individual layers
cd sedan-graph-ui/src/sedan-web && npm run lint     # sedan-web lint
cd sedan-graph-ui/src/sedan-web && npm run test:run # sedan-web unit tests
cd sedan-graph-ui/src/sedan-web && npm run build    # sedan-web production build
cargo test -p gql-core --lib                       # unit tests
cargo test -p gql-core --test end_to_end           # query pipeline
cargo test -p gql-core --test data_integrity       # mutation correctness
cargo test -p gql-core --test adversarial          # DoS / malformed inputs
cargo test -p gql-core --test stress               # scale + budget enforcement
cargo test -p gql-core --test ldbc                 # LDBC SNB benchmark queries
cargo test -p gql-module --test integration        # module command pipeline
cargo test -p gql-module --test compatibility      # wire format / serialization
cargo test -p gql-core --test tck                  # openCypher TCK (3903 scenarios)
cargo bench -p gql-core -- --test                  # compile + smoke-run benchmarks
```

---

## Test inventory

| Suite | File | Count | What it proves |
|-------|------|-------|----------------|
| gql-core unit | `gql-core/src/**` | 678 | Individual functions: parser, planner, executor, storage, budget, config |
| sedandelivery-bolt unit | `sedandelivery-bolt/src/**` | 399 | PackStream codec, message framing, Bolt state machine, bridge layer |
| gql-api unit | `gql-api/src/**` | 115 | API handler pure functions |
| gql-redis-store unit | `gql-redis-store/src/**` | 59 | Redis storage layer logic |
| gql-fabric-store unit | `gql-fabric-store/src/**` | 45 | Fabric storage layer logic |
| sedandelivery-bolt integration | `sedandelivery-bolt/tests/integration/` | 178 | Python neo4j driver: CRUD, transactions, paths, concurrency |
| End-to-end | `tests/end_to_end.rs` | 42 | Full pipeline: parse → plan → execute → result |
| Exists/interactive | `tests/exists_interactive.rs` | 6 | EXISTS subqueries and pattern predicates |
| **Data integrity** | `tests/data_integrity.rs` | 23 | Referential integrity, MERGE atomicity, property round-trips, label/ID invariants |
| **Stress** | `tests/stress.rs` | 8 | 10 K node load, `all_node_ids`, label scan at scale, budget enforcement |
| **Adversarial** | `tests/adversarial.rs` | 11 | 300-level deep expression, 1.1 MiB query, cartesian product budget, varlen termination, division-by-zero |
| **LDBC SNB** | `tests/ldbc/` | 37 | IC1–IC14 complex read queries (BFS, temporal, geographic, aggregation); IU1–8 inserts; ID1–8 deletes; ShortestPath operator |
| **Module integration** | `gql-module/tests/integration.rs` | 18 | GQL.QUERY / GQL.MUTATE pipeline, graph lifecycle, stats format, multi-graph isolation |
| **Compatibility** | `gql-module/tests/compatibility.rs` | 28 | JSON wire format: null, bool, int, float, string, list, map, node, edge, path encodings; column ordering |
| **TCK compliance** | `tests/tck/` | 3903 | 100% openCypher TCK (all 195 feature files, 3903 scenarios, 16 031 steps) |
| **Benchmarks** | `benches/` | 26 | Performance baselines (see [benchmarks.md](benchmarks.md)) |
| **Total (excluding TCK)** | | **1,673** | |
| **Total (including TCK)** | | **5,576** | |

Bold entries were added by issue #189.

---

## CI tiers

The `.github/workflows/ci.yml` runs these tiers automatically, plus a dedicated
`Sedan Web Validation` job for the frontend:

### Sedan Web Validation — every PR and push
```bash
cd sedan-graph-ui/src/sedan-web
npm ci
npm run lint
npm run test:run
npm run build
```
Confirms the React/TypeScript/Vite app installs cleanly, lints, passes its
Vitest suite, and produces a production build.

### Tier 1 — every PR (< 2 min)
```
cargo fmt --check
cargo clippy -p gql-core -- -D warnings
cargo test -p gql-core --lib
```
Blocks merge if formatting, warnings, or unit tests fail.

### Tier 2 — every PR (< 10 min)
```
cargo test -p gql-core --test end_to_end
cargo test -p gql-core --test exists_interactive
cargo test -p gql-core --test data_integrity
cargo test -p gql-core --test adversarial
cargo check --workspace
```
Validates the query pipeline and mutation correctness on every proposed change.

### Tier 3 — push to main (< 30 min)
```
cargo test -p gql-core --test tck          # 3903 scenarios
cargo test -p gql-core --test stress
cargo test -p gql-core --test ldbc
cargo test -p gql-module --test integration
cargo test -p gql-module --test compatibility
```
Full compliance and scale validation after merge.

### Tier 4 — push to main, artifact upload
```
cargo bench -p gql-core --bench parser   -- --output-format bencher
cargo bench -p gql-core --bench executor -- --output-format bencher
cargo bench -p gql-core --bench scale    -- --output-format bencher
```
Results uploaded as `benchmark-results` artifact. Download with:
```bash
gh run list --workflow ci.yml --branch main --limit 1
gh run download <run-id> --name benchmark-results
```

---

## Establishing a performance baseline

Criterion supports saved baselines for regression detection.

```bash
# 1. Save current performance as baseline "v1" (run after a clean build)
cargo bench -p gql-core -- --save-baseline v1

# 2. On a future release, compare against the saved baseline
cargo bench -p gql-core -- --baseline v1

# Output will flag regressions:
# node_scan/1000   change: [+14.2% +15.1% +16.0%]  (p = 0.00)
#                  Performance has regressed.
```

Baselines live in `target/criterion/` (git-ignored). To persist them across
machines, archive `target/criterion/` as a CI artifact and restore it in
future runs before `--baseline`.

To re-baseline after a deliberate performance change:
```bash
cargo bench -p gql-core -- --save-baseline v2
```

---

## What each test layer validates

### Data integrity (`tests/data_integrity.rs`)

Proves that every mutation operation maintains graph invariants:

- **Referential integrity**: DETACH DELETE removes all connected edges; no
  dangling edge endpoints remain after any deletion.
- **MERGE atomicity**: ON CREATE SET fires only on creation; ON MATCH SET fires
  only on match; double MERGE is idempotent.
- **Property round-trips**: int, float, bool, string, unicode, empty string all
  survive write → read. SET overwrites only the target property. REMOVE makes
  a property return null. `SET n = {…}` replaces all properties.
- **Label integrity**: SET n:Label makes the node scannable by that label;
  REMOVE n:Label excludes it; nodes with multiple labels are found by any.
- **ID uniqueness and stability**: all node IDs and edge IDs are unique; IDs
  do not change when other nodes are deleted.

### Adversarial queries (`tests/adversarial.rs`)

Proves the engine cannot be crashed or hung by malicious inputs:

- Parser rejects queries > 1 MiB (`QueryTooLarge`).
- Parser rejects 300-level nesting (`RecursionLimitExceeded`).
- Parser rejects empty queries (`EmptyQuery`).
- Cartesian products exceeding the row budget are aborted (`ResultSetTooLarge`).
- Unbounded variable-length paths on deep chains terminate (either error or
  bounded result — not a hang).
- Integer overflow, null bytes in strings, and division-by-zero are handled
  without panic.

### Stress tests (`tests/stress.rs`)

Proves correctness and budget enforcement at scale:

- 10 000 nodes can be loaded and `graph_stats` returns accurate counts.
- `all_node_ids` returns the exact set of created node IDs.
- Label scan at 600 nodes returns the correct labelled subset.
- Row-limit budget is enforced: a query that would produce 50 rows fails when
  the budget allows only 10 (`ResultSetTooLarge`).
- Cartesian product of 100 pairs is stopped at row budget 20.
- An expired deadline produces `Timeout` before any rows are yielded.
- 200-pair 1-hop traversal returns the correct count.

### LDBC SNB benchmark queries (`tests/ldbc/`)

Validates correctness against the industry-standard Social Network Benchmark
schema using a synthetic dataset:

- **IC1–IC14**: all 14 LDBC Interactive Complex Read query patterns (multi-hop
  BFS, temporal ORDER BY, geographic filter, aggregation, tag co-occurrence,
  likers, replies, path finding via ShortestPath operator).
- **IU1–IU8**: all 8 LDBC insert operations (Person, knows edge, Post, Comment,
  Forum, likes, HAS_MEMBER, HAS_MODERATOR).
- **ID1–ID8**: all 8 LDBC delete operations with cascade correctness.
- **Write atomicity**: no dangling edges after cascaded delete.

### Module integration (`gql-module/tests/integration.rs`)

Proves the Redis module command pipeline works end-to-end (using the same
code path as the module handlers, without requiring a live Redis):

- Graph lifecycle (create, duplicate-create error, delete, list).
- GQL.QUERY returns JSON-serialised rows in deterministic column order.
- GQL.MUTATE reports correct stats (nodes_created, edges_created, etc.).
- Error cases: parse error, missing graph, budget exceeded.
- In-process state persists across multiple commands.
- Multiple named graphs are fully isolated.

### Compatibility (`gql-module/tests/compatibility.rs`)

Proves the JSON wire format used by GQL.QUERY / GQL.MUTATE:

- Scalar encoding: null → JSON null; bool → true/false; int → JSON number
  (including i64::MIN and MAX); float → JSON number (NaN and Infinity are
  serialisation errors); string → JSON string.
- `__`-prefixed strings are escaped to avoid collision with internal tags.
- Lists, nested lists, and maps serialise as JSON arrays/objects.
- Node → `"__node:{id}"` tagged string.
- Edge → `"__edge:{id}"` tagged string.
- Path → `{"__path_nodes": [...], "__path_edges": [...]}`.
- Columns are returned in lexicographic key order (deterministic BTreeMap).

---

## TCK compliance

SedanDelivery achieves 100% openCypher TCK compliance: **3903/3903 scenarios**.

The TCK runner is in `gql-core/tests/tck/` and uses the `cucumber` framework
to execute Gherkin `.feature` files from `tests/tck/features/`.

```bash
# Run the full TCK (takes ~3 min)
cargo test -p gql-core --test tck

# Expected output:
# 195 features
# 3903 scenarios (3903 passed)
# 16031 steps (16031 passed)
```

If any scenario fails, Cucumber prints the failing scenario name, the failing
step, and the assertion error. The test binary exits with code 101 only when
scenarios actually fail — the Gherkin parse errors that existed before
[PR #208](https://github.com/cloud-ecosystem-security/SedanDelivery/pull/208)
have been fixed.

---

## Regression policy

| Trigger | Expected result |
|---------|----------------|
| New PR | Tier 1 + 2 must pass (green) before merge |
| Merge to main | Tier 3 must pass; Tier 4 uploads benchmark artifact |
| TCK count drops | Build failure (cargo test exits non-zero) |
| gql-core unit test count < 678 | Would indicate deleted tests — investigate |
| Benchmark regresses > 10% | Download artifact, run `--baseline` locally, investigate |

The TCK count is self-enforcing: if a previously-passing scenario starts
failing, `cargo test --test tck` exits non-zero and CI fails.

---

## Re-running from scratch

To reproduce the full validation on a fresh checkout:

```bash
git clone https://github.com/cloud-ecosystem-security/SedanDelivery.git
cd SedanDelivery

# Install Rust if needed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup default stable

# Node.js 22+ is also required for sedan-web validation

# Install libclang (required by redis-module bindgen)
# macOS:  brew install llvm
# Ubuntu: sudo apt-get install libclang-dev

# Run all validation tiers
bash scripts/validate.sh
```

---

## See also

- [benchmarks.md](benchmarks.md) — how to run and interpret performance benchmarks
- [ADR-001](ADR-001-storage-model.md) — storage model decisions
- [ADR-002](ADR-002-full-gql-compliance.md) — GQL compliance strategy
- [Architecture Overview](architecture.md) — crate map, request flows, storage layer
