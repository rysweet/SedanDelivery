## Bug: 12 parse_env_range/parse_env_min tuning knobs undocumented

**Layer**: runtime-topology × api-contracts
**Severity**: Medium
**Pass**: 2
**Evidence**:

12 environment variables parsed via the project's validated config helpers
(`parse_env_range`/`parse_env_min`) are missing from `docs/index.md`:

| Variable | File | Default |
|----------|------|---------|
| `GQL_MAX_COLLECT_SIZE` | gql-core/src/config.rs:104 | 10M |
| `GQL_MAX_LIST_SIZE` | gql-core/src/config.rs:105 | 10M |
| `GQL_MAX_REGEX_PATTERN_LEN` | gql-core/src/config.rs:107 | 1024 |
| `GQL_GRAPH_NAME_MAX_LEN` | gql-core/src/config.rs:112 | 64 |
| `GQL_PLAN_CACHE_SIZE` | gql-core/src/config.rs:116 | 1024 |
| `GQL_REGEX_CACHE_CAPACITY` | gql-core/src/config.rs:118 | 512 |
| `GQL_BUDGET_CHECK_INTERVAL` | gql-core/src/config.rs:124 | 1024 |
| `GQL_REDIS_STATS_INTERVAL_SECS` | gql-redis-store/src/connection.rs:27 | 15 |
| `GQL_REDIS_INDEX_BATCH_SIZE` | gql-redis-store/src/indexing.rs:25 | 500 |
| `GQL_REDIS_SMEMBERS_THRESHOLD` | gql-redis-store/src/reads.rs:21 | 10000 |
| `BOLT_TCP_KEEPALIVE_SECS` | sedandelivery-bolt/src/main.rs:150 | 120 |
| `GQL_CSV_IMPORT_DIR` | gql-core/src/config.rs:131 | cwd |

- code_quote: `parse_env_range("GQL_PLAN_CACHE_SIZE", 1024, 0, 1_000_000)`
- code_quote: `parse_env_min("GQL_MAX_REGEX_PATTERN_LEN", 1024, 1)`

**Impact**: Operators deploying at scale cannot discover these tuning knobs.
`GQL_MAX_REGEX_PATTERN_LEN` (ReDoS risk) and `GQL_MAX_COLLECT_SIZE` (OOM risk)
have security implications.

**Fix**: Add all 12 to the appropriate section of `docs/index.md`.
