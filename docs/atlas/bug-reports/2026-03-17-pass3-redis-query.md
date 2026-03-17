## Journey: redis-query

### Verdict: PASS

| Criterion | Status | Evidence |
|-----------|--------|----------|
| api-contracts routes match | pass | GQL.QUERY command registered at `gql-module/src/lib.rs:407` via `redis_module!` macro |
| data-flow complete | pass | cmd_gql_query at lib.rs:193 → parse_query → QueryPlanner → QueryExecutor → RESP array |
| runtime-topology matches | pass | Module runs inside Valkey process, no network hop needed |
| service-components reachable | pass | Single file module (lib.rs) calls gql-core directly |
| No dead code on critical path | pass | All query pipeline symbols are actively used |

**Verdict Rationale:** The Redis module journey is the simplest path. The `cmd_gql_query` handler
at `gql-module/src/lib.rs:193` is wrapped in `catch_unwind` for panic safety (lib.rs:194). The
read-only enforcement at lib.rs:204-206 correctly rejects write clauses by inspecting AST clauses.
The global InMemoryGraphStore at lib.rs:52 (`static STORE: OnceLock<ChangeLogStore<InMemoryGraphStore>>`)
provides the storage backend. All symbols on the path are verified active.
