# Bolt v4.4 Driver Conformance Tests

This document describes the driver-based conformance tests that validate
SedanDelivery's `sedandelivery-bolt` server against a real Neo4j Rust client
(`neo4rs`).

## Why a driver-based test?

Unit tests in `sedandelivery-bolt/src/protocol/` exercise the codec in
isolation. They cannot detect:

- Handshake / version negotiation bugs that only manifest when a real client
  performs the magic-number + 4×u32 protocol handshake.
- Chunk-framing regressions (16-bit chunk header, 0x0000 end-of-message
  marker, multi-chunk RECORD streams).
- `RUN` / `PULL` / `DISCARD` / `BEGIN` / `COMMIT` / `ROLLBACK` semantics that
  depend on the exact `qid` and `bookmarks` round-trip the driver expects.
- Driver-specific encoding assumptions (e.g. `neo4rs` decodes `Int` strictly
  as `i64`; if the server emits `Float` in a numeric context a downstream
  `get::<i64>()` will silently fail in surprising ways).
- Type mapping for nodes, relationships, paths, lists, maps, and temporals
  through both directions (param → server, result → client).

We also already run a `Bolt v4.4 Compatibility` workflow with the official
**Python** driver and a multi-instance test with the same Python driver.
This Rust-driver suite is intentionally a second, independent
implementation — if both pass, we have strong evidence the wire is correct;
if one passes and the other fails, we have isolated a driver-specific bug
worth investigating.

## What's covered

The suite (`tests/bolt-driver-conformance/tests/conformance.rs`) covers:

| Test | Surface exercised |
|------|-------------------|
| `handshake_and_trivial_query` | Magic + version handshake, HELLO, RUN, PULL, RECORD, SUCCESS |
| `multiple_sequential_queries_on_one_connection` | Connection reuse / state machine after each PULL |
| `many_parallel_connections` | Connection pool, concurrent sessions on the same server |
| `returns_primitive_scalar_types` | Int, Float, Bool, String, Null encoding |
| `returns_list_and_map` | PackStream List + Map structures |
| `create_and_return_node` | Node structure tag (`0x4E`), labels list, properties map |
| `create_and_return_relationship` | Relationship structure tag (`0x52`), type + props |
| `parameterized_query_with_mixed_types` | Param encoding round-trip for mixed types |
| `explicit_transaction_commit_persists_data` | BEGIN → RUN → COMMIT lifecycle, durability through txn |
| `explicit_transaction_rollback_discards_data` | BEGIN → RUN → ROLLBACK lifecycle, no side effects |
| `syntax_error_is_reported_via_failure` | FAILURE message surfaced as driver error |
| `large_result_set_streams_correctly` | 1500-row PULL n=-1 streaming, chunk framing under load |

## Running locally

```bash
bash scripts/run_bolt_conformance.sh
```

This script:

1. Builds `sedandelivery-bolt` in `release` mode (override with
   `CARGO_PROFILE=debug`).
2. Picks a random free TCP port via Python's socket library.
3. Boots the server on `127.0.0.1:<port>` with the **in-memory store**
   (no `GQL_REDIS_URL`) and **no auth** (no `BOLT_PASSWORD` —
   `neo4rs` connects with the default `neo4j` / `neo4j` credentials, which
   the server accepts when password auth is disabled).
4. Waits for the port to accept connections.
5. Runs `cargo test -p bolt-driver-conformance -- --ignored --test-threads=1`
   with `BOLT_URI=bolt://127.0.0.1:<port>` exported.
6. Tears the server down (logs are kept in `target/bolt-conformance-server.log`
   for post-mortem on failure).

All conformance tests are marked `#[ignore]` so that
`cargo test --workspace` does **not** require a running server.

## CI

The workflow `.github/workflows/bolt-driver-conformance.yml` runs the same
script on every PR that touches `sedandelivery-bolt/`, `cypher-core/src/`,
the conformance crate, or this script.

## Known gaps

If a Cypher type is *not* yet plumbed through Bolt, the test for it should
be removed (or scoped down) here and a follow-up issue filed — never papered
over with a weak assertion. Currently:

- **Temporal types** (`Date`, `Time`, `DateTime`, `Duration`, `Point`):
  the bridge supports encoding (`sedandelivery-bolt/src/bridge/temporal/`)
  but the conformance suite does not yet assert on them via `neo4rs`'s
  typed accessors. Tracked as future work — extend
  `tests/bolt-driver-conformance/tests/conformance.rs` once the typed
  decoder shape is settled.
- **Path** values: covered by unit tests in `sedandelivery-bolt/src/bridge/types/path.rs`
  but not yet exercised end-to-end through `neo4rs`.

When closing those gaps, **add a row to the coverage table above** and
**delete the corresponding bullet here** so this document never goes stale.

## Adding a new conformance test

1. Add a `#[tokio::test]` function in
   `tests/bolt-driver-conformance/tests/conformance.rs`.
2. Mark it `#[ignore]` so it only runs under the script.
3. Use the `connect()` / `connect_with_pool(n)` helpers; do **not** hard-code
   `bolt://localhost:7687` — read `BOLT_URI` via the helpers.
4. Generate label/property names with `unique_label()` so concurrent test
   runs don't collide on the shared in-memory store.
5. Run `bash scripts/run_bolt_conformance.sh` locally to confirm.
6. Add a row to the coverage table in this document.
