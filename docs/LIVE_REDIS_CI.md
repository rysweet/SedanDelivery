# Live Redis / Valkey CI

The `Live Redis/Valkey Tests` workflow
(`.github/workflows/live-redis.yml`) executes every `#[ignore]`-gated
integration test under `sd2-redis-store/tests/` against a real Valkey
deployment. These tests exercise code paths — Redis pipelining, MOVED
redirects, slot-aware key routing, and metadata replication — that the default
`cargo test` workspace run cannot cover because `sd2-redis-store` is excluded
from the workspace test set (it requires a live Redis to do anything useful).

The workflow is **opt-in / observability**: it runs on every PR and push that
touches `sd2-redis-store`, but it is *not* a required check. Promote it to
required only once it has demonstrated stability across several weeks of
green runs.

## Triggers

| Event              | When it fires                                                                                                  |
| ------------------ | -------------------------------------------------------------------------------------------------------------- |
| `pull_request`     | PRs to `main` that touch `sd2-redis-store/**`, `cypher-core/**`, the script, or this workflow                  |
| `push`             | Pushes to `main` that touch the same paths                                                                     |
| `workflow_dispatch`| Manual run from the Actions UI (use this to validate cluster code on demand without opening a PR)              |

## Jobs

### `standalone`

Boots a single `valkey/valkey:8` container via the GitHub Actions
`services:` block, exports `GQL_REDIS_URL=redis://127.0.0.1:6379`, and runs
the `cluster_write_path` and `metadata_audit` `--ignored` test files. These
files only need a single-node Redis (the file name is historical).

### `cluster`

Boots three `valkey/valkey:8` containers with `cluster-enabled yes`, forms
a cluster via `redis-cli --cluster create`, exports
`GQL_REDIS_CLUSTER_NODES=redis://127.0.0.1:7000,redis://127.0.0.1:7001,redis://127.0.0.1:7002`,
and runs the `cluster_pipeline` test file — the only `--ignored` test file
that genuinely requires cluster mode (it asserts
`GQL_REDIS_CLUSTER_NODES` is set). The other `--ignored` files
(`cluster_write_path`, `metadata_audit`) read `GQL_REDIS_URL` and run in
the standalone job above.

The cluster job uses the helper script
`scripts/run_live_redis_tests.sh` because the `services:` block cannot
express the `redis-cli --cluster create` step.

## Running locally

You need Docker. Then, from the repo root:

```bash
bash scripts/run_live_redis_tests.sh
```

The script:

1. Starts a standalone Valkey container on port `6399` and runs the
   standalone-compatible `--ignored` tests.
2. Starts three Valkey containers (ports `7000`, `7001`, `7002`), forms a
   cluster, and runs the full `--ignored` suite against it.
3. Cleans up containers on exit (even on failure / Ctrl-C) via `trap`.

### Useful overrides

| Variable              | Default            | Purpose                                                                |
| --------------------- | ------------------ | ---------------------------------------------------------------------- |
| `VALKEY_IMAGE`        | `valkey/valkey:8`  | Use a different image (e.g. `redis:7`) to confirm cross-engine parity  |
| `CLUSTER_PORT_BASE`   | `7000`             | First cluster port (uses `BASE`, `BASE+1`, `BASE+2`)                   |
| `STANDALONE_PORT`     | `6399`             | Host port for the standalone container                                 |
| `SKIP_STANDALONE`     | unset              | Set to `1` to skip the standalone phase                                 |
| `SKIP_CLUSTER`        | unset              | Set to `1` to skip the cluster phase                                    |
| `KEEP_CONTAINERS`     | unset              | Set to `1` to leave containers running after the script exits (debug)   |
| `CARGO_TEST_FLAGS`    | empty              | Extra flags passed to `cargo test` after `--`                           |

Examples:

```bash
# Just the cluster phase, debug a specific test, keep containers running
SKIP_STANDALONE=1 KEEP_CONTAINERS=1 \
  CARGO_TEST_FLAGS="cluster_mode_pipelined_reads_and_index_backfill_succeed" \
  bash scripts/run_live_redis_tests.sh

# Run against upstream Redis 7 instead of Valkey 8
VALKEY_IMAGE=redis:7 bash scripts/run_live_redis_tests.sh
```

## Relationship to `redis-integration.yml`

The existing `.github/workflows/redis-integration.yml` runs a *narrower*
subset of `--ignored` tests (one file from `cluster_write_path`, plus the
full `cluster_pipeline`) alongside Bolt gallery compatibility checks.
`live-redis.yml` complements it by running the **full** `--ignored` set
against both topologies — every test gated behind `#[ignore = "requires…"]`
in `sd2-redis-store/tests/`.

Both workflows should stay green; if one fails and the other doesn't, the
delta is the surface to investigate first.

## Troubleshooting

- **`docker: command not found`** — install Docker (or Docker Desktop /
  Colima / Rancher Desktop). The script exits with code `2` early if
  Docker is missing.
- **`port already allocated`** — set `STANDALONE_PORT` and/or
  `CLUSTER_PORT_BASE` to free ports.
- **Cluster never reaches `cluster_state:ok`** — typically a port collision
  on the cluster bus port (`port + 10000`). Pick a different
  `CLUSTER_PORT_BASE`.
- **Tests fail with `MOVED` errors** — that's a real bug in the
  cluster-mode key routing in `sd2-redis-store`; capture the test output
  and file it as a regression.
